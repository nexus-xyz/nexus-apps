//! Compact review text and inline formatting, parsed once when the watch changes.

use std::ops::Range;

use markdown::mdast::Node;

pub(super) const MAX: usize = 280;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub link: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub range: Range<usize>,
    pub style: Style,
}

#[derive(Default)]
pub(super) struct Excerpt {
    pub text: String,
    pub marks: Vec<Mark>,
}

impl Excerpt {
    pub fn of(body: &str) -> Self {
        let mut excerpt = Self::default();
        match markdown::to_mdast(body, &markdown::ParseOptions::gfm()) {
            Ok(node) => excerpt.read(&node, Style::default()),
            Err(_) => excerpt.push(body, Style::default()),
        }
        excerpt.text.truncate(excerpt.text.trim_end().len());
        let mut styled_end = excerpt.text.len();
        if excerpt.text.chars().count() > MAX {
            let end = excerpt
                .text
                .char_indices()
                .nth(MAX - 1)
                .map_or(excerpt.text.len(), |(i, _)| i);
            // Keep whole words when possible, but still bound an unbroken identifier.
            let prefix = &excerpt.text[..end];
            let end = prefix.rfind(' ').filter(|i| *i > end / 2).unwrap_or(end);
            excerpt.text.truncate(end);
            styled_end = end;
            excerpt.text.push('…');
        }
        excerpt.marks.retain_mut(|mark| {
            mark.range.end = mark.range.end.min(styled_end);
            mark.range.start < mark.range.end
        });
        excerpt
    }

    fn push(&mut self, text: &str, style: Style) {
        let start = self.text.len();
        for ch in text.chars() {
            if ch.is_whitespace() {
                self.space();
            } else {
                self.text.push(ch);
            }
        }
        if style != Style::default() && start < self.text.len() {
            self.marks.push(Mark {
                range: start..self.text.len(),
                style,
            });
        }
    }

    fn space(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with(' ') {
            self.text.push(' ');
        }
    }

    fn read(&mut self, node: &Node, mut style: Style) {
        // One extra character tells us whether the preview needs an ellipsis.
        if self.text.chars().count() > MAX {
            return;
        }
        match node {
            Node::Text(text) => self.push(&text.value, style),
            Node::InlineCode(code) => self.push(&code.value, Style { code: true, ..style }),
            Node::Code(code) => {
                self.space();
                self.push(&code.value, Style { code: true, ..style });
                self.space();
            }
            Node::Image(image) => self.push(&image.alt, style),
            Node::ImageReference(image) => self.push(&image.alt, style),
            Node::Break(_) => self.space(),
            Node::Html(_) | Node::Definition(_) | Node::ThematicBreak(_) => {}
            Node::Heading(heading) => {
                let label = heading.children.iter().map(Node::to_string).collect::<String>();
                if !matches!(
                    label.trim().to_lowercase().as_str(),
                    "summary" | "review" | "details" | "detail"
                ) {
                    self.space();
                    for child in &heading.children {
                        self.read(child, Style { strong: true, ..style });
                    }
                    self.space();
                }
            }
            _ => {
                let block = matches!(node, Node::Paragraph(_) | Node::ListItem(_) | Node::TableRow(_));
                if block {
                    self.space();
                }
                match node {
                    Node::Strong(_) => style.strong = true,
                    Node::Emphasis(_) => style.emphasis = true,
                    Node::Delete(_) => style.deleted = true,
                    Node::Link(_) | Node::LinkReference(_) => style.link = true,
                    _ => {}
                }
                if let Some(children) = node.children() {
                    for child in children {
                        self.read(child, style);
                    }
                }
                if block {
                    self.space();
                }
            }
        }
    }
}
