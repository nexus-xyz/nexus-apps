use super::*;

/// How a team's row is drawn in the tree: each level gets its own division.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A top-level team with teams under it: a band with a rule above it, over its cards.
    Band,
    /// A second-level team with teams under it: the head of its own card.
    Head,
    /// A row on a card with a hairline above it: a third-level team, or a team with nothing under it.
    Area,
    /// Deeper: it hangs off a tree guide.
    Leaf,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    /// The team's index in the flattened tree.
    pub(super) row: usize,
    pub(super) kind: Kind,
    /// `Some(open)` when teams sit under it, so it folds.
    pub(super) fold: Option<bool>,
    /// The teams under it, at any depth.
    pub(super) below: usize,
    /// A leaf's extra indent steps, past the first leaf level.
    pub(super) indent: usize,
    /// The last leaf under its parent: its guide stops at its middle.
    pub(super) last: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Block {
    Band(Line),
    /// One raised card. Its first line is a `Head`, or the first of a run of sibling teams with nothing under them.
    Card(Vec<Line>),
}

/// The tree as bands and cards. `rows` is the flattened tree (parents first); `folded` holds the slugs folded shut.
pub(super) fn layout(rows: &[TreeRow], folded: &HashSet<String>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let top = rows.iter().map(|row| row.depth).min().unwrap_or(1);
    place(rows, 0, rows.len(), top, folded, &mut blocks);
    blocks
}

/// The teams under `rows[at]`, at any depth.
fn below(rows: &[TreeRow], at: usize) -> usize {
    rows[at + 1..]
        .iter()
        .take_while(|row| row.depth > rows[at].depth)
        .count()
}

fn fold(rows: &[TreeRow], at: usize, under: usize, folded: &HashSet<String>) -> Option<bool> {
    (under > 0).then(|| !folded.contains(&rows[at].slug))
}

/// Lay out the siblings in `rows[start..end]`: a top-level team (at depth `top`) with teams under it opens a band, a
/// deeper one a card. Siblings with nothing under them share one card, so a flat organization reads as one list.
fn place(rows: &[TreeRow], start: usize, end: usize, top: usize, folded: &HashSet<String>, out: &mut Vec<Block>) {
    let mut run: Vec<Line> = Vec::new();
    let mut at = start;
    while at < end {
        let under = below(rows, at);
        let line = |kind| Line {
            row: at,
            kind,
            fold: fold(rows, at, under, folded),
            below: under,
            indent: 0,
            last: false,
        };
        if under == 0 {
            run.push(line(Kind::Area));
        } else {
            if !run.is_empty() {
                out.push(Block::Card(std::mem::take(&mut run)));
            }
            if rows[at].depth <= top {
                out.push(Block::Band(line(Kind::Band)));
                if !folded.contains(&rows[at].slug) {
                    place(rows, at + 1, at + 1 + under, top, folded, out);
                }
            } else {
                out.push(Block::Card(card(rows, at, under, folded)));
            }
        }
        at += 1 + under;
    }
    if !run.is_empty() {
        out.push(Block::Card(run));
    }
}

/// A second-level team and the open teams under it, on one card.
fn card(rows: &[TreeRow], head: usize, under: usize, folded: &HashSet<String>) -> Vec<Line> {
    let depth = rows[head].depth;
    let end = head + 1 + under;
    let mut lines = vec![Line {
        row: head,
        kind: Kind::Head,
        fold: fold(rows, head, under, folded),
        below: under,
        indent: 0,
        last: false,
    }];
    if folded.contains(&rows[head].slug) {
        return lines;
    }
    let mut at = head + 1;
    while at < end {
        let row = &rows[at];
        let mine = below(rows, at);
        let next = at + 1 + mine;
        let area = row.depth == depth + 1;
        lines.push(Line {
            row: at,
            kind: if area { Kind::Area } else { Kind::Leaf },
            fold: fold(rows, at, mine, folded),
            below: mine,
            indent: row.depth.saturating_sub(depth + 2),
            last: !area && (next >= end || rows[next].depth < row.depth),
        });
        at = if mine > 0 && folded.contains(&row.slug) {
            next
        } else {
            at + 1
        };
    }
    lines
}
