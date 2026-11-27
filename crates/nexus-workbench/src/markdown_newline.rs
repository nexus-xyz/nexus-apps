//! Code composer list continuation, matching `markdown-newline.ts`.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub insert: String,
    pub caret: usize,
}

fn in_fence(text: &str) -> bool {
    let mut fence = None;
    for line in text.lines() {
        let line = line.trim_start();
        let Some(marker @ (b'`' | b'~')) = line.as_bytes().first().copied() else {
            continue;
        };
        let length = line.bytes().take_while(|byte| *byte == marker).count();
        if length < 3 {
            continue;
        }
        match fence {
            None => fence = Some((marker, length)),
            Some((open, width)) if marker == open && length >= width => fence = None,
            _ => {}
        }
    }
    fence.is_some()
}

/// Byte offsets match GPUI's text selection. Invalid boundaries leave the input alone.
pub fn markdown_newline(text: &str, start: usize, end: usize) -> Option<TextEdit> {
    if start == 0 || end < start || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return None;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    if in_fence(&text[..line_start]) || text.get(start..end)?.contains('\n') {
        return None;
    }
    let prefix = &text[line_start..start];
    let indent_len = prefix.bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let indent = &prefix[..indent_len];
    let rest = &prefix[indent_len..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let marker_len = if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
        digits + 1
    } else if matches!(rest.as_bytes().first(), Some(b'-' | b'+' | b'*')) {
        1
    } else {
        return None;
    };
    let after = &rest[marker_len..];
    let spaces = after.bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
    if spaces == 0 {
        return None;
    }
    let space = &after[..spaces];
    let mut content = &after[spaces..];
    let mut task_space = None;
    if content.len() >= 4
        && content.as_bytes()[0] == b'['
        && matches!(content.as_bytes()[1], b' ' | b'x' | b'X')
        && content.as_bytes()[2] == b']'
    {
        let n = content[3..].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
        if n > 0 {
            task_space = Some(&content[3..3 + n]);
            content = &content[3 + n..];
        }
    }
    let tail = text.get(end..)?;
    if content.trim().is_empty() && tail.split('\n').next().unwrap_or_default().trim().is_empty() {
        return Some(TextEdit {
            range: line_start..end,
            insert: indent.into(),
            caret: line_start + indent.len(),
        });
    }
    let marker = if digits > 0 {
        let mut bytes = rest.as_bytes()[..digits].to_vec();
        let mut carry = true;
        for digit in bytes.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            bytes.insert(0, b'1');
        }
        format!("{}{}", String::from_utf8(bytes).ok()?, rest.as_bytes()[digits] as char)
    } else {
        rest[..marker_len].into()
    };
    let insert = format!(
        "\n{indent}{marker}{space}{}",
        task_space.map(|space| format!("[ ]{space}")).unwrap_or_default()
    );
    Some(TextEdit {
        range: start..end,
        caret: start + insert.len(),
        insert,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit(text: &str, start: usize, end: usize) -> Option<(String, usize)> {
        let edit = markdown_newline(text, start, end)?;
        Some((
            format!(
                "{}{}{}",
                &text[..edit.range.start],
                edit.insert,
                &text[edit.range.end..]
            ),
            edit.caret,
        ))
    }
    fn at_end(text: &str) -> Option<String> {
        edit(text, text.len(), text.len()).map(|(text, _)| text)
    }
    #[test]
    fn lists_preserve_markers_indentation_and_number_width() {
        for (before, after) in [
            ("1. First item", "1. First item\n2. "),
            ("009) Item", "009) Item\n010) "),
            ("  - Nested item", "  - Nested item\n  - "),
            ("\t* Item", "\t* Item\n\t* "),
            ("+ Item", "+ Item\n+ "),
            (
                "99999999999999999999. Big",
                "99999999999999999999. Big\n100000000000000000000. ",
            ),
        ] {
            assert_eq!(at_end(before).as_deref(), Some(after));
        }
    }
    #[test]
    fn tasks_start_unchecked_and_empty_items_end() {
        for (before, after) in [
            ("- [x] Done", "- [x] Done\n- [ ] "),
            ("3. [ ] Pending", "3. [ ] Pending\n4. [ ] "),
            ("1. First\n2. ", "1. First\n"),
            ("  - [ ] ", "  "),
            ("* ", ""),
        ] {
            assert_eq!(at_end(before).as_deref(), Some(after));
        }
    }
    #[test]
    fn selection_and_unicode_caret_are_byte_correct() {
        assert_eq!(edit("1. Hello world", 8, 8), Some(("1. Hello\n2.  world".into(), 12)));
        assert_eq!(edit("- one replace two", 5, 13), Some(("- one\n-  two".into(), 8)));
        assert_eq!(at_end("- café").as_deref(), Some("- café\n- "));
        assert!(markdown_newline("- é", 3, 3).is_none());
    }
    #[test]
    fn ordinary_lines_and_code_fences_stay_unchanged() {
        for text in [
            "ordinary text",
            "1.not a list",
            "```text\n1. in code",
            "~~~\n- in code",
            "```rust\ncode\n~~~\n1. still code",
            "````\ncode\n```\n1. still code",
        ] {
            assert!(at_end(text).is_none(), "{text}");
        }
        assert_eq!(
            at_end("```\ncode\n```\n1. outside").as_deref(),
            Some("```\ncode\n```\n1. outside\n2. ")
        );
        assert!(markdown_newline("- one\n- two", 3, 8).is_none());
    }
}
