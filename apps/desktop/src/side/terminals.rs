//! The Workbench's terminals, as the Nexus app's (`terminal-set.ts`): a strip of terminals,
//! each with up to four shells side by side (a split), "+" for a new one, double click to rename, and a close on
//! each. Hiding the Workbench keeps shells running; a primary close can retain them outside these views. With one terminal the
//! strip hides (the Workbench's own "Terminal" tab names it, and its header has "+" and split); it shows from two up.
//! Before a session starts, the set is drawn as the Commands tab the session gets once it starts (`commands.rs`).

mod commands;
#[cfg(test)]
mod commands_tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub use commands::render_empty as render_empty_commands;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::INSET;
use super::terminal::{RetainedTerminal, ServerTarget, Terminal};

/// Shells in one split, as the Nexus app's (`MAX_SPLIT`).
pub const MAX_SPLIT: usize = 4;

type ShellFactory = fn(PathBuf, &mut Context<Terminal>) -> Terminal;
type ServerShellFactory = fn(ServerTarget, &mut Context<Terminal>) -> Terminal;

/// A server session's set: its shells open on its server, not in a folder here.
#[derive(Clone)]
struct OnServer {
    target: ServerTarget,
    factory: ServerShellFactory,
}

/// One terminal in the strip: its name and its shells, side by side.
struct Group {
    id: usize,
    name: SharedString,
    shells: Vec<Entity<Terminal>>,
    /// The shell that had the keys last, which a new split goes after and the strip focuses.
    focused: usize,
}

/// Terminal layout and processes retained without any window-bound view state.
pub struct RetainedTerminalSet {
    dir: PathBuf,
    groups: Vec<RetainedGroup>,
    shell_factory: ShellFactory,
    server: Option<OnServer>,
    active: usize,
    next: usize,
}

struct RetainedGroup {
    id: usize,
    name: SharedString,
    shells: Vec<RetainedTerminal>,
    focused: usize,
}

/// The last terminal closed: the Workbench drops its Terminal tab.
pub enum TerminalSetEvent {
    Empty,
}

pub struct TerminalSet {
    /// The folder its shells start in; for a server's set, only the name the Workbench keeps it under.
    dir: PathBuf,
    groups: Vec<Group>,
    shell_factory: ShellFactory,
    server: Option<OnServer>,
    active: usize,
    /// The next terminal's number, for its id and its name.
    next: usize,
    /// The terminal being renamed, with its field.
    renaming: Option<(usize, Entity<InputState>, Subscription)>,
    /// Drawn as the Commands tab (`commands.rs`): a session's shells before it starts.
    commands: bool,
    /// In the Commands look, the terminals whose rows are closed.
    collapsed: HashSet<usize>,
    /// In the Commands look, each row's well: its height, its drag and the maximized one (`side::well`).
    wells: super::well::Wells,
    /// The shell that joined a split last: it slides in once (`motion::slide_in`).
    arrived: Option<EntityId>,
    /// In the Commands look, each split's shares of its well's width, by terminal id, after the person dragged a
    /// divider. A split with no entry shares it equally.
    shares: HashMap<usize, Vec<f32>>,
}

impl EventEmitter<TerminalSetEvent> for TerminalSet {}

impl TerminalSet {
    /// A set in `dir`, with one terminal.
    pub fn new(dir: PathBuf, cx: &mut Context<Self>) -> TerminalSet {
        Self::with_shell_factory(dir, Terminal::new, cx)
    }

    #[cfg(test)]
    pub(crate) fn for_view_test(dir: PathBuf, cx: &mut Context<Self>) -> TerminalSet {
        Self::with_shell_factory(dir, Terminal::for_view_test, cx)
    }

    /// A server session's set, with one shell on its server. `key` names it in the Workbench.
    pub fn on_server(key: PathBuf, target: ServerTarget, cx: &mut Context<Self>) -> TerminalSet {
        Self::with_server(key, target, Terminal::on_server, cx)
    }

    #[cfg(test)]
    pub(crate) fn server_for_view_test(key: PathBuf, target: ServerTarget, cx: &mut Context<Self>) -> TerminalSet {
        Self::with_server(key, target, Terminal::server_for_view_test, cx)
    }

    fn with_server(
        key: PathBuf,
        target: ServerTarget,
        factory: ServerShellFactory,
        cx: &mut Context<Self>,
    ) -> TerminalSet {
        let mut set = Self::empty(key, Terminal::new);
        set.server = Some(OnServer { target, factory });
        set.add(cx);
        set
    }

    /// Whether its shells run on a server.
    #[cfg(test)]
    pub(crate) fn on_a_server(&self) -> bool {
        self.server.is_some()
    }

    /// Each terminal's name, and the command its first shell runs on a server, if any.
    #[cfg(test)]
    pub(crate) fn listed(&self, cx: &App) -> Vec<(String, Option<String>)> {
        self.groups
            .iter()
            .map(|group| {
                let command = group
                    .shells
                    .first()
                    .and_then(|shell| shell.read(cx).server_command().map(str::to_string));
                (group.name.to_string(), command)
            })
            .collect()
    }

    fn empty(dir: PathBuf, shell_factory: ShellFactory) -> TerminalSet {
        TerminalSet {
            dir,
            groups: Vec::new(),
            shell_factory,
            server: None,
            active: 0,
            next: 1,
            renaming: None,
            commands: false,
            collapsed: HashSet::new(),
            wells: Default::default(),
            arrived: None,
            shares: HashMap::new(),
        }
    }

    fn with_shell_factory(dir: PathBuf, shell_factory: ShellFactory, cx: &mut Context<Self>) -> TerminalSet {
        let mut set = Self::empty(dir, shell_factory);
        set.add(cx);
        set
    }

    /// Move all shell owners out; rename input and subscriptions belong to the closing window.
    pub fn retain(&mut self, cx: &mut Context<Self>) -> RetainedTerminalSet {
        self.renaming = None;
        RetainedTerminalSet {
            dir: self.dir.clone(),
            groups: std::mem::take(&mut self.groups)
                .into_iter()
                .map(|group| RetainedGroup {
                    id: group.id,
                    name: group.name,
                    shells: group
                        .shells
                        .into_iter()
                        .map(|shell| shell.update(cx, |shell, _| shell.retain()))
                        .collect(),
                    focused: group.focused,
                })
                .collect(),
            shell_factory: self.shell_factory,
            server: self.server.clone(),
            active: self.active,
            next: self.next,
        }
    }

    pub fn restore(saved: RetainedTerminalSet, cx: &mut Context<Self>) -> Self {
        TerminalSet {
            dir: saved.dir,
            groups: saved
                .groups
                .into_iter()
                .map(|group| Group {
                    id: group.id,
                    name: group.name,
                    shells: group
                        .shells
                        .into_iter()
                        .map(|shell| cx.new(|cx| Terminal::restore(shell, cx)))
                        .collect(),
                    focused: group.focused,
                })
                .collect(),
            shell_factory: saved.shell_factory,
            server: saved.server,
            active: saved.active,
            next: saved.next,
            renaming: None,
            commands: false,
            collapsed: HashSet::new(),
            wells: Default::default(),
            arrived: None,
            shares: HashMap::new(),
        }
    }

    /// Draw the set as the Commands tab, or as terminals side by side.
    pub fn set_commands_look(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.commands != on {
            self.commands = on;
            for shell in self.groups.iter().flat_map(|group| &group.shells) {
                shell.update(cx, |shell, cx| shell.set_well(on, cx));
            }
            cx.notify();
        }
    }

    /// A new shell in the set's folder, or on its server, drawn for the set's look.
    fn new_shell(&self, cx: &mut Context<Self>) -> Entity<Terminal> {
        let (dir, shell_factory, well) = (self.dir.clone(), self.shell_factory, self.commands);
        let server = self.server.clone();
        cx.new(|cx| {
            let mut shell = match server {
                Some(server) => (server.factory)(server.target, cx),
                None => shell_factory(dir, cx),
            };
            shell.set_well(well, cx);
            shell
        })
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// Whether the chosen terminal has room for another shell beside its own.
    pub fn can_split(&self) -> bool {
        self.groups
            .get(self.active)
            .is_some_and(|group| group.shells.len() < MAX_SPLIT)
    }

    /// A new terminal at the end of the strip, chosen.
    pub fn add(&mut self, cx: &mut Context<Self>) {
        let id = self.next;
        self.next += 1;
        let shell = self.new_shell(cx);
        self.groups.push(Group {
            id,
            name: format!("Terminal {id}").into(),
            shells: vec![shell],
            focused: 0,
        });
        self.active = self.groups.len() - 1;
        cx.notify();
    }

    /// Run `command` in a terminal named `name`, else after the command: a new one when `add`, else the chosen one. It
    /// is typed into the shell, so the shell stays for what comes next.
    pub fn run(&mut self, command: &str, name: Option<&str>, add: bool, cx: &mut Context<Self>) {
        if self.server.is_some() {
            self.run_on_server(command, name, add, cx);
            return;
        }
        if add || self.groups.is_empty() {
            self.add(cx);
        }
        let Some(group) = self.groups.get_mut(self.active) else {
            return;
        };
        group.name = name.map_or_else(|| command_name(command), str::to_string).into();
        if let Some(shell) = group.shells.get(group.focused) {
            shell.update(cx, |shell, _| shell.type_command(command));
        }
        cx.notify();
    }

    /// Run `command` once in a new terminal, chosen, and hand back its shell to watch its end
    /// ([`super::terminal::TerminalEvent::Exited`]): a setup fix's step. Here it runs in `cwd`, else the set's folder;
    /// on a server, on the server.
    pub fn run_watched(
        &mut self,
        command: &str,
        name: Option<&str>,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Entity<Terminal> {
        let well = self.commands;
        let server = self.server.clone();
        let dir = cwd.unwrap_or_else(|| self.dir.clone());
        let shell = cx.new(|cx| {
            let mut shell = match server {
                Some(server) => {
                    let mut target = server.target;
                    target.command = Some(command.to_string());
                    let mut shell = (server.factory)(target, cx);
                    shell.watch_exit();
                    shell
                }
                None => Terminal::run_once(dir, command, cx),
            };
            shell.set_well(well, cx);
            shell
        });
        let id = self.next;
        self.next += 1;
        self.groups.push(Group {
            id,
            name: name.map_or_else(|| command_name(command), str::to_string).into(),
            shells: vec![shell.clone()],
            focused: 0,
        });
        self.active = self.groups.len() - 1;
        cx.notify();
        shell
    }

    /// [`TerminalSet::run`] on a server: the command runs as the shell itself (`$SHELL -lc`), as the Nexus app's
    /// shared terminal runs a project command, so its exit code shows when it ends; Enter then starts a shell. Not
    /// `add`: the set's first shell, which nothing used yet, gives way to it.
    fn run_on_server(&mut self, command: &str, name: Option<&str>, add: bool, cx: &mut Context<Self>) {
        let Some(server) = self.server.clone() else {
            return;
        };
        let mut target = server.target;
        target.command = Some(command.to_string());
        let well = self.commands;
        let shell = cx.new(|cx| {
            let mut shell = (server.factory)(target, cx);
            shell.set_well(well, cx);
            shell
        });
        let name: SharedString = name.map_or_else(|| command_name(command), str::to_string).into();
        let fresh = !add && self.groups.len() == 1 && self.groups[0].shells.len() == 1;
        if fresh {
            let group = &mut self.groups[0];
            group.name = name;
            group.shells = vec![shell];
            group.focused = 0;
            self.active = 0;
        } else {
            let id = self.next;
            self.next += 1;
            self.groups.push(Group {
                id,
                name,
                shells: vec![shell],
                focused: 0,
            });
            self.active = self.groups.len() - 1;
        }
        cx.notify();
    }

    /// Another shell beside the chosen terminal's, up to [`MAX_SPLIT`].
    pub fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_split() {
            return;
        }
        let shell = self.new_shell(cx);
        let arrived = shell.entity_id();
        let Some(group) = self.groups.get_mut(self.active) else {
            return;
        };
        let at = (group.focused + 1).min(group.shells.len());
        group.shells.insert(at, shell);
        group.focused = at;
        // A new shell shares the well equally with the others.
        self.shares.remove(&group.id);
        self.arrived = Some(arrived);
        self.focus(window, cx);
        cx.notify();
    }

    /// The Commands row's Split: another shell in terminal `ix`'s well.
    fn split_group(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.groups.len() {
            self.active = ix;
            self.split(window, cx);
        }
    }

    /// Move the divider after shell `divider` of terminal `id` to `x` px from the left of its `width` px well, in
    /// whole columns of `cell` px.
    fn drag_divider(&mut self, id: usize, divider: usize, x: f32, width: f32, cell: f32, cx: &mut Context<Self>) {
        let Some(group) = self.groups.iter().find(|group| group.id == id) else {
            return;
        };
        let shares = self.shares_of(group);
        if let Some(moved) = move_divider(&shares, divider, x, width, cell)
            && moved != shares
        {
            self.shares.insert(id, moved);
            cx.notify();
        }
    }

    /// Each shell's share of terminal `group`'s well.
    fn shares_of(&self, group: &Group) -> Vec<f32> {
        let count = group.shells.len().max(1);
        self.shares
            .get(&group.id)
            .filter(|shares| shares.len() == count)
            .cloned()
            .unwrap_or_else(|| vec![1. / count as f32; count])
    }

    /// Give the keys to the chosen terminal's last shell.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.get(self.active)
            && let Some(shell) = group.shells.get(group.focused)
        {
            // In the Commands look, the row of the shell that takes the keys opens, so the shell shows.
            self.collapsed.remove(&group.id);
            let handle = shell.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
    }

    /// Whether one of the shells has the keys.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.groups.iter().any(|group| {
            group
                .shells
                .iter()
                .any(|shell| shell.read(cx).focus_handle().contains_focused(window, cx))
        })
    }

    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.groups.len() {
            self.active = ix;
            self.focus(window, cx);
            cx.notify();
        }
    }

    fn close_group(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.groups.len() {
            return;
        }
        let closed = self.groups.remove(ix);
        self.shares.remove(&closed.id);
        if self.wells.is_maximized(&commands::well_key(closed.id)) {
            self.wells.restore();
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|(id, ..)| !self.groups.iter().any(|g| g.id == *id))
        {
            self.renaming = None;
        }
        if self.groups.is_empty() {
            cx.emit(TerminalSetEvent::Empty);
            return;
        }
        if self.active >= ix && self.active > 0 {
            self.active -= 1;
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn close_shell(&mut self, group_ix: usize, shell_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get_mut(group_ix) else {
            return;
        };
        if group.shells.len() <= 1 {
            self.close_group(group_ix, window, cx);
            return;
        }
        group.shells.remove(shell_ix);
        group.focused = group.focused.min(group.shells.len() - 1);
        self.shares.remove(&group.id);
        self.focus(window, cx);
        cx.notify();
    }

    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(ix) else {
            return;
        };
        let (id, name) = (group.id, group.name.clone());
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        let name = input.read(cx).value().trim().to_string();
                        if let Some(group) = this.groups.iter_mut().find(|group| group.id == id)
                            && !name.is_empty()
                        {
                            group.name = name.into();
                        }
                        this.renaming = None;
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.focus(window, cx);
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.renaming = Some((id, input, subscription));
        cx.notify();
    }

    fn render_strip(&self, palette: crate::palette::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.groups.iter().enumerate().map(|(ix, group)| {
            let chosen = ix == self.active;
            let renaming = self
                .renaming
                .as_ref()
                .filter(|(id, ..)| *id == group.id)
                .map(|(_, input, _)| input.clone());
            let split = group.shells.len();
            let hover_group: SharedString = format!("terminal-group-{}", group.id).into();
            h_flex()
                .id(("terminal-group", group.id))
                .group(hover_group.clone())
                .h(px(26.))
                .pl(px(8.))
                .pr(px(3.))
                .gap(px(5.))
                .rounded(px(6.))
                .text_size(px(12.5))
                .cursor_pointer()
                .when(chosen, |tab| tab.bg(palette.selected).text_color(palette.foreground))
                .when(!chosen, |tab| {
                    tab.text_color(palette.muted)
                        .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.choose(ix, window, cx);
                    }
                }))
                .child(Icon::new(Lucide::SquareTerminal).size(px(13.)))
                .map(|tab| match renaming {
                    Some(input) => tab.child(div().w(px(110.)).child(Input::new(&input).xsmall())),
                    None => tab.child(div().max_w(px(140.)).truncate().child(group.name.clone())),
                })
                .when(split > 1, |tab| {
                    tab.child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.faint)
                            .child(format!("×{split}")),
                    )
                })
                .child(
                    div()
                        .id(("terminal-group-close", group.id))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        // As the Workbench's tabs: the × shows on the chosen tab and under the pointer.
                        .when(!chosen, |close| {
                            close
                                .invisible()
                                .group_hover(hover_group.clone(), |style| style.visible())
                        })
                        .hover(|style| style.bg(if chosen { palette.hover } else { palette.selected }))
                        .tooltip(|window, cx| Tooltip::new("Close the terminal").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_group(ix, window, cx);
                        }))
                        .child(Icon::new(Lucide::X).size(px(11.)).text_color(palette.faint)),
                )
        });
        // The tabs' text starts where the Workbench's tabs' does: 12px in, less a tab's own 8px.
        h_flex()
            .id("terminal-strip")
            .flex_none()
            .h(px(34.))
            .pl(px(INSET - 8.))
            .pr(px(INSET))
            .gap(px(4.))
            .overflow_x_scroll()
            .children(tabs.collect::<Vec<_>>())
    }
}

impl Render for TerminalSet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        // The shell with the keys, so a split and the strip's focus go back to it.
        let active = self.active;
        if let Some(group) = self.groups.get_mut(active)
            && let Some(ix) = group
                .shells
                .iter()
                .position(|shell| shell.read(cx).focus_handle().contains_focused(window, cx))
        {
            group.focused = ix;
        }
        if self.commands {
            return self.render_commands(palette, window, cx);
        }
        let body = self.groups.get(active).map(|group| {
            let many = group.shells.len() > 1;
            h_flex()
                .flex_1()
                .min_h(px(0.))
                .children(group.shells.iter().enumerate().map(|(ix, shell)| {
                    div()
                        .id(("terminal-shell", ix))
                        .group("terminal-shell")
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        // Shells side by side part on a 6px gutter of the transcript's fill: a gap, not a hairline.
                        .when(ix > 0, |cell| cell.border_l(px(6.)).border_color(palette.background))
                        .child(shell.clone())
                        .when(many, |cell| {
                            cell.child(
                                div()
                                    .id(("terminal-shell-close", ix))
                                    .absolute()
                                    .top(px(4.))
                                    .right(px(4.))
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.))
                                    .bg(palette.sidebar)
                                    .invisible()
                                    .group_hover("terminal-shell", |style| style.visible())
                                    .hover(|style| style.bg(palette.hover))
                                    .cursor_pointer()
                                    .tooltip(|window, cx| Tooltip::new("Close this shell").build(window, cx))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_shell(active, ix, window, cx);
                                    }))
                                    .child(Icon::new(Lucide::X).size(px(11.)).text_color(palette.faint)),
                            )
                        })
                }))
        });
        let strip = (self.groups.len() > 1).then(|| self.render_strip(palette, cx));
        v_flex().size_full().children(strip).children(body).into_any_element()
    }
}

/// The fewest columns a shell in a split keeps when its divider is dragged.
const MIN_COLUMNS: f32 = 20.;

/// `shares` (each shell's part of a `width` px well, summing to 1) with the divider after shell `divider` moved to
/// `x` px from the well's left edge. The shell before it takes whole columns of `cell` px, and both shells beside it
/// keep at least `MIN_COLUMNS`. None when there is no such divider, or no room to move it.
fn move_divider(shares: &[f32], divider: usize, x: f32, width: f32, cell: f32) -> Option<Vec<f32>> {
    if divider + 1 >= shares.len() || width <= 0. || cell <= 0. {
        return None;
    }
    let before: f32 = shares[..divider].iter().sum::<f32>() * width;
    let pair = (shares[divider] + shares[divider + 1]) * width;
    let least = MIN_COLUMNS * cell;
    if pair < 2. * least {
        return None;
    }
    let left = ((x - before) / cell).round() * cell;
    let left = left.clamp(least, pair - least);
    let mut moved = shares.to_vec();
    moved[divider] = left / width;
    moved[divider + 1] = (pair - left) / width;
    Some(moved)
}

/// What a new terminal running `command` is called: its first line, cut to fit the strip.
fn command_name(command: &str) -> String {
    const LONGEST: usize = 40;
    let line = command.trim().lines().next().unwrap_or_default().trim();
    let more = line.chars().count() > LONGEST || command.trim().lines().nth(1).is_some();
    let mut name: String = line.chars().take(LONGEST).collect();
    if more {
        name.push('…');
    }
    name
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[gpui_kit::test]
    fn retaining_recreates_views_and_preserves_groups_splits_and_selection(cx: &mut TestAppContext) {
        let set = cx.new(|cx| TerminalSet::for_view_test(std::env::temp_dir(), cx));
        let (first_view, second_view) = set.update(cx, |set, cx| {
            set.groups[0].name = "Kept name".into();
            set.groups[0]
                .shells
                .push(cx.new(|cx| Terminal::for_view_test(std::env::temp_dir(), cx)));
            set.groups[0].focused = 1;
            set.add(cx);
            (set.groups[0].shells[0].entity_id(), set.groups[0].shells[1].entity_id())
        });
        let saved = set.update(cx, |set, cx| set.retain(cx));
        assert!(set.read_with(cx, |set, _| set.groups.is_empty()));
        drop(set);
        let restored = cx.new(|cx| TerminalSet::restore(saved, cx));
        restored.update(cx, |set, cx| {
            assert_eq!(set.groups.len(), 2);
            assert_eq!(set.groups[0].name.as_ref(), "Kept name");
            assert_eq!(set.groups[0].shells.len(), 2);
            assert_eq!(set.groups[0].focused, 1);
            assert_eq!(set.active, 1);
            assert_eq!(set.next, 3);
            assert_ne!(set.groups[0].shells[0].entity_id(), first_view);
            assert_ne!(set.groups[0].shells[1].entity_id(), second_view);
            assert!(set.renaming.is_none());
            set.add(cx);
            assert_eq!(set.groups[2].id, 3);
        });
    }

    /// A host that is never reached: the view-test shells open nothing.
    struct Unreached;

    impl crate::side::terminal::ShellHost for Unreached {
        fn call(
            &self,
            _: &'static str,
            _: serde_json::Value,
        ) -> futures::future::BoxFuture<'static, Result<serde_json::Value, String>> {
            Box::pin(async { Err("no server in tests".to_string()) })
        }
    }

    #[gpui_kit::test]
    fn a_project_command_on_a_server_runs_as_a_shell_of_its_own_there(cx: &mut TestAppContext) {
        let target = ServerTarget {
            host: std::sync::Arc::new(Unreached),
            thread: Some("t1".into()),
            project: None,
            command: None,
        };
        let set = cx.new(|cx| TerminalSet::server_for_view_test("nexus-server://box/t1".into(), target, cx));
        set.update(cx, |set, cx| {
            // The set's first shell, unused, gives way to the first command.
            set.run("npm test", Some("Test"), false, cx);
            assert_eq!(set.listed(cx), vec![("Test".into(), Some("npm test".into()))]);
            // Another opens beside it, named after its command when it has no name.
            set.run("cargo build\ncargo test", None, true, cx);
            assert_eq!(
                set.listed(cx),
                vec![
                    ("Test".into(), Some("npm test".into())),
                    ("cargo build…".into(), Some("cargo build\ncargo test".into())),
                ]
            );
            assert_eq!(set.active, 1);
            // A new terminal is an interactive shell.
            set.add(cx);
            assert_eq!(set.listed(cx)[2].1, None);
        });
    }

    #[test]
    fn a_divider_moves_in_whole_columns_and_leaves_each_shell_twenty() {
        let even = [0.5, 0.5];
        // A 400px well of 8px columns: the divider at 203px lands on column 25, 200px.
        let moved = move_divider(&even, 0, 203., 400., 8.).unwrap();
        assert_eq!(moved, vec![0.5, 0.5]);
        let moved = move_divider(&even, 0, 229., 400., 8.).unwrap();
        assert_eq!(moved, vec![232. / 400., 168. / 400.]);
        // Past either end, each shell keeps 20 columns, 160px.
        assert_eq!(move_divider(&even, 0, 10., 400., 8.).unwrap(), vec![0.4, 0.6]);
        assert_eq!(move_divider(&even, 0, 390., 400., 8.).unwrap(), vec![0.6, 0.4]);
        // Three shells: the middle divider moves only the two beside it.
        let third = 1. / 3.;
        let moved = move_divider(&[third, third, third], 1, 424., 600., 8.).unwrap();
        assert!((moved[0] - third).abs() < 1e-6);
        assert!((moved[1] - 224. / 600.).abs() < 1e-6, "{moved:?}");
        assert!((moved.iter().sum::<f32>() - 1.).abs() < 1e-6);
        // No divider after the last shell, and no room for two shells of 20 columns.
        assert_eq!(move_divider(&even, 1, 200., 400., 8.), None);
        assert_eq!(move_divider(&even, 0, 100., 200., 8.), None);
    }

    #[test]
    fn a_terminal_is_named_after_its_command() {
        assert_eq!(command_name("  npm test\n"), "npm test");
        assert_eq!(command_name("cd app\nnpm test"), "cd app…");
        assert_eq!(command_name(&"x".repeat(50)), format!("{}…", "x".repeat(40)));
    }
}
