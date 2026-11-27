//! Files dragged from the system over a session browser's page, as Chrome hands them to a website: an HTML drag and
//! drop with the files (an upload area, say). GPUI reports such a drag as moves anywhere in the window and a drop; this
//! turns them into the page's enter, over, leave and drop. The host passes them to Chromium (`host.rs`).
//!
//! Only a session browser takes them. On the app's own pages (artifacts, Nexus pages) a drop still attaches the files to
//! the composer, as anywhere else in the window.

use std::path::PathBuf;

use super::protocol::FileDragStep;

/// The files over the page now, if any.
#[derive(Default)]
pub(super) struct FileDrag {
    over: Option<Vec<PathBuf>>,
}

impl FileDrag {
    /// The drag of `paths` moved, `inside` the page or not: the steps to send.
    pub(super) fn moved(&mut self, paths: &[PathBuf], inside: bool) -> Vec<FileDragStep> {
        match (&self.over, inside) {
            (Some(over), true) if over.as_slice() == paths => vec![FileDragStep::Over],
            // Other files: the last drag ended where the view did not see it (outside the window).
            (Some(_), true) => {
                self.over = Some(paths.to_vec());
                vec![FileDragStep::Leave, enter(paths)]
            }
            (None, true) => {
                self.over = Some(paths.to_vec());
                vec![enter(paths)]
            }
            (Some(_), false) => self.left(),
            (None, false) => Vec::new(),
        }
    }

    /// The drag left the page.
    pub(super) fn left(&mut self) -> Vec<FileDragStep> {
        match self.over.take() {
            Some(_) => vec![FileDragStep::Leave],
            None => Vec::new(),
        }
    }

    /// `paths` were dropped on the page.
    pub(super) fn dropped(&mut self, paths: &[PathBuf]) -> Vec<FileDragStep> {
        let mut steps = self.moved(paths, true);
        steps.retain(|step| *step != FileDragStep::Over);
        self.over = None;
        steps.push(FileDragStep::Drop);
        steps
    }

    pub(super) fn active(&self) -> bool {
        self.over.is_some()
    }
}

fn enter(paths: &[PathBuf]) -> FileDragStep {
    FileDragStep::Enter {
        paths: paths.iter().map(|path| path.to_string_lossy().into_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_drag_enters_moves_over_and_drops() {
        let mut drag = FileDrag::default();
        let files = paths(&["/tmp/a.png"]);
        // Outside the page first: nothing.
        assert!(drag.moved(&files, false).is_empty());
        assert_eq!(
            drag.moved(&files, true),
            vec![FileDragStep::Enter {
                paths: vec!["/tmp/a.png".into()]
            }]
        );
        assert!(drag.active());
        assert_eq!(drag.moved(&files, true), vec![FileDragStep::Over]);
        assert_eq!(drag.dropped(&files), vec![FileDragStep::Drop]);
        assert!(!drag.active());
    }

    #[test]
    fn a_drag_that_leaves_the_page_leaves_once() {
        let mut drag = FileDrag::default();
        let files = paths(&["/tmp/a.png", "/tmp/b.txt"]);
        drag.moved(&files, true);
        assert_eq!(drag.moved(&files, false), vec![FileDragStep::Leave]);
        assert!(drag.moved(&files, false).is_empty());
        assert!(drag.left().is_empty());
        // Back over the page: it enters again.
        assert!(matches!(drag.moved(&files, true)[..], [FileDragStep::Enter { .. }]));
        assert_eq!(drag.left(), vec![FileDragStep::Leave]);
    }

    #[test]
    fn a_new_drag_ends_one_the_view_lost() {
        let mut drag = FileDrag::default();
        drag.moved(&paths(&["/tmp/a.png"]), true);
        // The first drag left the window without a move outside the page; a second one comes in.
        let steps = drag.moved(&paths(&["/tmp/c.pdf"]), true);
        assert_eq!(
            steps,
            vec![
                FileDragStep::Leave,
                FileDragStep::Enter {
                    paths: vec!["/tmp/c.pdf".into()]
                }
            ]
        );
    }

    #[test]
    fn a_drop_with_no_move_before_it_enters_first() {
        let mut drag = FileDrag::default();
        let steps = drag.dropped(&paths(&["/tmp/a.png"]));
        assert_eq!(
            steps,
            vec![
                FileDragStep::Enter {
                    paths: vec!["/tmp/a.png".into()]
                },
                FileDragStep::Drop
            ]
        );
    }
}
