//! A row of links at the top of each answer in a Code session, as the Nexus app's
//! (`answer-links.tsx`): under the person's message, the pages the turn published (open in the Artifacts tab,
//! as a page card does), up to three files it wrote with lines added and removed (open the turn in Changes, at the
//! file), the media it saved under `.nexus/artifacts/` (an image opens full size, a video in the system's player),
//! then "+N files" (open the turn in Changes). No row when the turn made nothing.
//!
//! Pages are the transcript's [`Block::Page`]s; media are the answer's Markdown links. Files come from the turn's
//! checkpoint (`nexus_workbench::turn_files`), read from git here or through the server's `wb_turn_files`, so they show
//! once the turn has ended. A server's session shows no media chips: its files are not on this computer.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::changes_source::ChangesSource;
use nexus_workbench::turn_files::TurnFile;

use super::slash::Glyph;
use super::{View, Workspace};
use crate::code::Block;
use crate::file_icons::FileIcon;
use crate::palette::Palette;

/// The files the row names one by one; the rest are "+N files".
const SHOWN_FILES: usize = 3;

/// Where an agent saves screenshots and videos (`nexus_workbench::output_media::ARTIFACTS_DIR`).
const ARTIFACTS: &str = ".nexus/artifacts/";

/// A Markdown link's or image's destination.
static LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"!?\[[^\]\n]*\]\(\s*<?([^)\s>]+)>?").expect("a valid pattern"));

/// What the answer to a message made, read from the blocks up to the next message.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Answer {
    /// The turn's `turnEnd` event, once it ended: its files can be read.
    pub turn: Option<i64>,
    /// The pages, as (id, title).
    pub pages: Vec<(String, Option<String>)>,
    /// The media paths, as the answer wrote them.
    pub media: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Chip {
    Page { page_id: String, title: Option<String> },
    File(TurnFile),
    Media(String),
    More(usize),
}

/// The turns' files read so far, by turn; None while one is being read.
#[derive(Default)]
pub(super) struct AnswerFiles(HashMap<i64, Option<Vec<TurnFile>>>);

/// The answer to the message at block `ix`: the pages, media and turn end in the blocks after it.
pub(super) fn answer_of(blocks: &[Block], ix: usize) -> Answer {
    let mut answer = Answer::default();
    for block in blocks.iter().skip(ix + 1) {
        match block {
            Block::User { .. } => break,
            Block::Turn { seq, .. } => {
                answer.turn = Some(*seq);
                break;
            }
            Block::Page { page_id, title, .. } => match answer.pages.iter_mut().find(|(id, _)| id == page_id) {
                Some((_, known)) if known.is_none() => known.clone_from(title),
                Some(_) => {}
                None => answer.pages.push((page_id.clone(), title.clone())),
            },
            Block::Text(text) => {
                for path in media_in(text) {
                    if !answer.media.contains(&path) {
                        answer.media.push(path);
                    }
                }
            }
            _ => {}
        }
    }
    answer
}

/// The row's chips, in order: pages, up to three files, media, then "+N files". Empty: no row.
pub(super) fn chips(answer: &Answer, files: &[TurnFile]) -> Vec<Chip> {
    let pages = answer.pages.iter().map(|(page_id, title)| Chip::Page {
        page_id: page_id.clone(),
        title: title.clone(),
    });
    let shown = files.iter().take(SHOWN_FILES).cloned().map(Chip::File);
    let media = answer.media.iter().cloned().map(Chip::Media);
    let more = (files.len() > SHOWN_FILES).then(|| Chip::More(files.len() - SHOWN_FILES));
    pages.chain(shown).chain(media).chain(more).collect()
}

/// The images and videos under `.nexus/artifacts/` an answer links to or shows, as the Nexus app's `mediaReference`:
/// local paths (or `file:` URLs) without a line, each once.
pub(super) fn media_in(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for link in LINK.captures_iter(text) {
        let target = &link[1];
        let path = target.strip_prefix("file://").unwrap_or(target);
        let local = !path.contains("://") && !path.contains('#') && !path.contains('?');
        if local && path.contains(ARTIFACTS) && media_kind(path).is_some() && !found.iter().any(|seen| seen == path) {
            found.push(path.to_string());
        }
    }
    found
}

/// Whether a path is an image or a video, by its extension (`inline-media-model.ts`'s `mediaKind`).
fn media_kind(path: &str) -> Option<bool> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Some(false),
        "mp4" | "webm" | "mov" => Some(true),
        _ => None,
    }
}

/// The message block a row of links belongs to for block `ix`: the last message at or before it, else `ix`. A block
/// that changes there changes that message's row, so its item is measured again (`CodeView::set`).
pub(super) fn row_of(blocks: &[Block], ix: usize) -> usize {
    blocks
        .iter()
        .take(ix + 1)
        .rposition(|block| matches!(block, Block::User { .. }))
        .unwrap_or(ix)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Workspace {
    /// Read the files of the open Code session's ended turns not read yet, off the UI thread. Called after each read.
    pub(super) fn load_answer_files(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        if !view.loaded || view.is_draft() {
            return;
        }
        let turns: Vec<i64> = view
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Turn { seq, .. } if !view.answer_files.0.contains_key(seq) => Some(*seq),
                _ => None,
            })
            .collect();
        if turns.is_empty() {
            return;
        }
        let thread = view.id.clone();
        let machines = crate::machines::get();
        let machine = machines.machine_of(&thread);
        let folder = match (&machine, &self.code) {
            (None, Some(code)) => code.folder(&thread).ok().map(|folder| folder.path),
            _ => None,
        };
        if machine.is_none() && folder.is_none() {
            return;
        }
        for turn in &turns {
            view.answer_files.0.insert(*turn, None);
        }
        let read = cx.background_executor().spawn({
            let thread = thread.clone();
            async move {
                let mut read = Vec::with_capacity(turns.len());
                for turn in turns {
                    // No checkpoint (a folder outside git, a turn before checkpoints): no files, and nothing to say.
                    let files = match (&machine, &folder) {
                        (Some(machine), _) => {
                            let args = serde_json::json!({ "threadId": thread, "turn": turn });
                            match machines.call(machine, "wb_turn_files", args).await {
                                Ok(Ok(answer)) => serde_json::from_value(answer).unwrap_or_default(),
                                _ => Vec::new(),
                            }
                        }
                        (None, Some(dir)) => {
                            nexus_workbench::turn_files::turn_files(dir, &thread, turn).unwrap_or_default()
                        }
                        (None, None) => Vec::new(),
                    };
                    read.push((turn, files));
                }
                read
            }
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                if view.id != thread {
                    return;
                }
                for (turn, files) in read {
                    // The message the turn answers shows them: its item is measured again.
                    let ended = view
                        .blocks
                        .iter()
                        .position(|block| matches!(block, Block::Turn { seq, .. } if *seq == turn));
                    if let Some(ix) = ended.map(|ix| row_of(&view.blocks, ix))
                        && matches!(view.blocks.get(ix), Some(Block::User { .. }))
                        && !files.is_empty()
                    {
                        view.list.remeasure_items(ix..ix + 1);
                    }
                    view.answer_files.0.insert(turn, Some(files));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The row of links under the message at block `ix` of the open Code session; None when its answer made nothing.
    pub(super) fn render_answer_links(
        &self,
        ix: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let View::Code(view) = &self.view else {
            return None;
        };
        if !matches!(view.blocks.get(ix), Some(Block::User { .. })) {
            return None;
        }
        let mut answer = answer_of(&view.blocks, ix);
        // A server's media are not on this computer.
        if crate::machines::get().machine_of(&view.id).is_some() {
            answer.media.clear();
        }
        let files = answer
            .turn
            .and_then(|turn| view.answer_files.0.get(&turn).cloned().flatten())
            .unwrap_or_default();
        let chips = chips(&answer, &files);
        if chips.is_empty() {
            return None;
        }
        let turn = answer.turn;
        let mut drawn = Vec::with_capacity(chips.len());
        for (n, chip) in chips.into_iter().enumerate() {
            drawn.push(
                self.render_answer_chip(ix, n, chip, turn, palette, cx)
                    .into_any_element(),
            );
        }
        let row = h_flex().flex_wrap().gap(px(6.)).pt(px(10.)).children(drawn);
        Some(row.into_any_element())
    }

    fn render_answer_chip(
        &self,
        ix: usize,
        n: usize,
        chip: Chip,
        turn: Option<i64>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (icon, label, counts): (Option<Glyph>, _, _) = match &chip {
            Chip::Page { title, .. } => (
                Some(Lucide::AppWindow.into()),
                title.clone().unwrap_or_else(|| "Nexus page".into()),
                None,
            ),
            Chip::File(file) => (
                Some(Glyph::File(FileIcon::file(&file.path))),
                file_name(&file.path).to_string(),
                Some((file.added, file.removed)),
            ),
            Chip::Media(path) => (
                Some(
                    if media_kind(path) == Some(true) {
                        Lucide::Film
                    } else {
                        Lucide::Image
                    }
                    .into(),
                ),
                file_name(path).to_string(),
                None,
            ),
            Chip::More(count) => (
                None,
                format!("+{count} {}", if *count == 1 { "file" } else { "files" }),
                None,
            ),
        };
        let tip: SharedString = match &chip {
            Chip::Page { .. } => "Open the page beside the conversation".into(),
            Chip::File(file) => format!("{}: open in Changes", file.path).into(),
            Chip::Media(path) => path.clone().into(),
            Chip::More(_) => "Open this turn in Changes".into(),
        };
        let more = matches!(chip, Chip::More(_));
        div()
            .id(SharedString::from(format!("answer-chip-{ix}-{n}")))
            .h(px(26.))
            .max_w(px(260.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(13.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .text_size(px(12.))
            .text_color(if more { palette.muted } else { palette.foreground })
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .when_some(icon, |chip, icon| {
                chip.child(match icon {
                    Glyph::Icon(icon) => Icon::new(icon)
                        .size(px(13.))
                        .text_color(palette.muted)
                        .into_any_element(),
                    Glyph::File(icon) => img(icon.asset(palette.dark))
                        .flex_none()
                        .size(px(13.))
                        .into_any_element(),
                })
            })
            .child(div().min_w(px(0.)).truncate().child(label))
            .when_some(counts, |chip, (added, removed)| {
                chip.when(added > 0, |chip| {
                    chip.child(div().text_color(gpui_kit::rgb(0x3a8f3e)).child(format!("+{added}")))
                })
                .when(removed > 0, |chip| {
                    chip.child(div().text_color(palette.destructive).child(format!("−{removed}")))
                })
            })
            .on_click(cx.listener(move |this, _, window, cx| this.open_answer_chip(&chip, turn, window, cx)))
    }

    fn open_answer_chip(&mut self, chip: &Chip, turn: Option<i64>, window: &mut Window, cx: &mut Context<Self>) {
        match chip {
            Chip::Page { page_id, .. } => self.open_code_page(page_id, false, cx),
            Chip::File(file) => {
                if let Some(turn) = turn {
                    self.focus_changes(ChangesSource::Turn(turn), Some(file.path.clone()), window, cx);
                }
            }
            Chip::More(_) => {
                if let Some(turn) = turn {
                    self.focus_changes(ChangesSource::Turn(turn), None, window, cx);
                }
            }
            Chip::Media(path) => self.open_answer_media(path, cx),
        }
    }

    /// An image full size, as an image sent in the chat opens; a video in the system's player.
    fn open_answer_media(&mut self, path: &str, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            return;
        };
        let Some(folder) = self.code.as_ref().and_then(|code| code.folder(&view.id).ok()) else {
            return;
        };
        let full = folder.path.join(path);
        if media_kind(path) == Some(true) {
            cx.open_with_system(&full);
            return;
        }
        let Ok(bytes) = std::fs::read(&full) else {
            return;
        };
        let name = file_name(path).to_string();
        let image = crate::attachments::Attachment {
            artifact: None,
            id: 0,
            media_type: crate::attachments::image_type(&name),
            name,
            bytes: Arc::new(bytes),
            thumbnail: None,
        };
        self.open_image(&[image], 0, cx);
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI Kit's glob brings its own `test` attribute.
    use super::{Answer, Block, Chip, TurnFile, answer_of, chips, media_in, row_of};
    use nexus_workbench::event::TurnStatus;
    use nexus_workbench::turn_files::FileStatus;

    fn user(seq: i64) -> Block {
        Block::User {
            seq,
            at: 0,
            text: "Do it".into(),
            from: None,
            rewound: false,
            images: 0,
            image_data: Vec::new(),
        }
    }

    fn turn(seq: i64) -> Block {
        Block::Turn {
            seq,
            at: 0,
            status: TurnStatus::Completed,
            duration_ms: 1,
            cost_usd: None,
            tokens: 0,
            model: None,
            reverted: false,
        }
    }

    fn page(id: &str, title: Option<&str>) -> Block {
        Block::Page {
            page_id: id.into(),
            title: title.map(str::to_string),
            turn: 1,
            seq: 2,
            written: true,
        }
    }

    fn file(path: &str) -> TurnFile {
        TurnFile {
            path: path.into(),
            added: 2,
            removed: 1,
            status: FileStatus::Modified,
        }
    }

    fn blocks() -> Vec<Block> {
        vec![
            user(1),
            page("pag_1", Some("Q3 spend")),
            Block::Text(
                "See ![login](.nexus/artifacts/login.png), [flow](./.nexus/artifacts/flow.webm) and \
                 [notes](docs/a.png)."
                    .into(),
            ),
            page("pag_2", None),
            turn(6),
            user(7),
            Block::Text("Working on it.".into()),
        ]
    }

    #[test]
    fn an_answer_holds_its_pages_media_and_turn_up_to_the_next_message() {
        assert_eq!(
            answer_of(&blocks(), 0),
            Answer {
                turn: Some(6),
                pages: vec![("pag_1".into(), Some("Q3 spend".into())), ("pag_2".into(), None)],
                media: vec![
                    ".nexus/artifacts/login.png".into(),
                    "./.nexus/artifacts/flow.webm".into()
                ],
            }
        );
        // A turn still running has no end yet, so no files.
        assert_eq!(answer_of(&blocks(), 5), Answer::default());
    }

    #[test]
    fn the_chips_go_pages_files_media_then_the_rest_of_the_files() {
        let answer = answer_of(&blocks(), 0);
        let files: Vec<TurnFile> = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"].map(file).into();
        let kinds: Vec<&str> = chips(&answer, &files)
            .iter()
            .map(|chip| match chip {
                Chip::Page { .. } => "page",
                Chip::File(_) => "file",
                Chip::Media(_) => "media",
                Chip::More(_) => "more",
            })
            .collect();
        assert_eq!(
            kinds,
            ["page", "page", "file", "file", "file", "media", "media", "more"]
        );
        assert_eq!(chips(&answer, &files).last(), Some(&Chip::More(2)));
        assert!(
            !chips(&answer, &files[..3])
                .iter()
                .any(|chip| matches!(chip, Chip::More(_)))
        );
    }

    #[test]
    fn an_answer_that_made_nothing_has_no_row() {
        assert!(chips(&answer_of(&blocks(), 5), &[]).is_empty());
    }

    #[test]
    fn media_are_local_images_and_videos_under_the_artifacts_folder() {
        let text = "![a](.nexus/artifacts/a.PNG) ![b](https://x.com/.nexus/artifacts/b.png) \
                    [c](.nexus/artifacts/c.txt) [d](file:///home/me/app/.nexus/artifacts/d.mp4) \
                    [e](.nexus/artifacts/e.png#L3) ![a again](.nexus/artifacts/a.PNG)";
        assert_eq!(
            media_in(text),
            [".nexus/artifacts/a.PNG", "/home/me/app/.nexus/artifacts/d.mp4"]
        );
    }

    #[test]
    fn a_change_in_an_answer_measures_its_message_again() {
        let blocks = blocks();
        assert_eq!(row_of(&blocks, 3), 0);
        assert_eq!(row_of(&blocks, 6), 5);
        assert_eq!(row_of(&blocks[1..], 1), 1);
    }
}
