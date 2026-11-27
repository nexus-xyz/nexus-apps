//! The actual side composer, using its isolated Runs source and the test platform's clipboard.
use super::{draw, panel};
use gpui_kit::{ClipboardItem, Focusable, TestAppContext};

const THRESHOLD: usize = 32 * 1024;

fn paste_key(literal: bool) -> &'static str {
    match (cfg!(target_os = "macos"), literal) {
        (true, false) => "cmd-v",
        (true, true) => "cmd-shift-v",
        (false, false) => "ctrl-v",
        (false, true) => "ctrl-shift-v",
    }
}

#[gpui_kit::test]
fn large_side_pastes_attach_exact_bytes_and_reserve_names_without_changing_the_prompt(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.input
            .update(cx, |input, cx| input.set_value("Question", window, cx));
        chat.input.focus_handle(cx).focus(window, cx);
        window.activate_window();
    });
    draw(cx);
    // The exact byte boundary takes the same path as pasting an ordinary clipboard item in the textarea.
    let text = "x".repeat(THRESHOLD);
    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    cx.simulate_keystrokes(paste_key(false));
    cx.simulate_keystrokes(paste_key(false));
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), "Question");
        assert_eq!(chat.draft.files.len(), 2);
        assert_eq!(chat.draft.files[0].name, "pasted-text.txt");
        assert_eq!(chat.draft.files[1].name, "pasted-text-2.txt");
        assert_eq!(chat.draft.files[0].bytes.as_slice(), text.as_bytes());
        assert_eq!(chat.draft.files[1].bytes.as_slice(), text.as_bytes());
        assert!(!chat.sending);
    });
    // Multibyte characters reach the limit before the character count does.
    let unicode = "界".repeat(THRESHOLD / 3 + 1);
    chat.update(cx, |chat, cx| {
        assert!(chat.paste(&ClipboardItem::new_string(unicode.clone()), cx))
    });
    chat.read_with(cx, |chat, _| {
        assert_eq!(chat.draft.files[2].name, "pasted-text-3.txt");
        assert_eq!(chat.draft.files[2].bytes.as_slice(), unicode.as_bytes());
    });
}

#[gpui_kit::test]
fn literal_side_paste_replaces_the_selection_once_and_short_paste_stays_inline(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.input.update(cx, |input, cx| {
            input.set_value("before OLD after", window, cx);
            input.set_selected_range(7..10, cx);
        });
        chat.input.focus_handle(cx).focus(window, cx);
        window.activate_window();
    });
    draw(cx);
    let text = "z".repeat(THRESHOLD);
    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    cx.simulate_keystrokes(paste_key(true));
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), format!("before {text} after"));
        assert!(chat.draft.is_empty());
        assert!(!chat.sending);
    });
    chat.update_in(cx, |chat, window, cx| {
        chat.input.update(cx, |input, cx| input.set_value("", window, cx))
    });
    let short = "s".repeat(THRESHOLD - 1);
    cx.write_to_clipboard(ClipboardItem::new_string(short.clone()));
    cx.simulate_keystrokes(paste_key(false));
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), short);
        assert!(chat.draft.is_empty());
    });
}

#[gpui_kit::test]
fn side_text_conversion_respects_pending_source_and_permission_guards(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let item = ClipboardItem::new_string("x".repeat(THRESHOLD));
    chat.update(cx, |chat, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        // As the session composer: a send in flight has left the composer, so the next message takes a paste.
        chat.sending = true;
        assert!(chat.paste(&item, cx));
        chat.sending = false;
        chat.draft = Default::default();
        chat.request = Some("approval".into());
        assert!(!chat.paste(&item, cx));
        chat.request = None;
        chat.secret_id = Some("secret".into());
        assert!(!chat.paste(&item, cx));
        chat.secret_id = None;
        chat.retired = true;
        assert!(!chat.paste(&item, cx));
        chat.retired = false;
        chat.pending.revoke();
        assert!(!chat.paste(&item, cx));
        assert!(chat.draft.is_empty());
    });
}

#[gpui_kit::test]
fn a_full_side_draft_reports_the_shared_limit_and_keeps_existing_attachments(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let item = ClipboardItem::new_string("x".repeat(THRESHOLD));
    chat.update(cx, |chat, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        for _ in 0..nexus_workbench::attachments::MAX_FILES {
            assert!(chat.paste(&item, cx));
        }
        let ids: Vec<_> = chat.draft.files.iter().map(|file| file.id).collect();
        assert!(chat.paste(&item, cx), "refused text must not fill the prompt");
        assert_eq!(chat.draft.files.iter().map(|file| file.id).collect::<Vec<_>>(), ids);
        assert!(
            chat.problem
                .as_ref()
                .is_some_and(|problem| problem.contains("10 files"))
        );
    });
}

#[gpui_kit::test]
fn literal_paste_is_scoped_to_the_side_prompt_and_obeys_permission_guards(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let key = gpui_kit::Keystroke::parse(paste_key(true)).unwrap();
    cx.write_to_clipboard(ClipboardItem::new_string("synthetic fixture text".into()));
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.model_input.focus_handle(cx).focus(window, cx);
        assert!(!chat.paste_as_text(&key, window, cx));
        chat.input.focus_handle(cx).focus(window, cx);
        chat.request = Some("approval".into());
        assert!(!chat.paste_as_text(&key, window, cx));
        chat.request = None;
        chat.secret_id = Some("secret".into());
        assert!(!chat.paste_as_text(&key, window, cx));
        assert!(chat.input.read(cx).value().is_empty());
    });
}

#[gpui_kit::test]
fn a_side_paste_with_an_image_keeps_even_large_text_inline(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let chat = panel.read_with(cx, |panel, _| panel.chat.clone());
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    let text = "mixed".repeat(THRESHOLD / 5 + 1);
    let mut item = ClipboardItem::new_string(text.clone());
    item.entries
        .push(gpui_kit::ClipboardEntry::Image(gpui_kit::Image::from_bytes(
            gpui_kit::ImageFormat::Png,
            bytes.into_inner(),
        )));
    chat.update_in(cx, |chat, window, cx| {
        chat.poll = None;
        chat.partial_poll = None;
        chat.input.focus_handle(cx).focus(window, cx);
        window.activate_window();
    });
    draw(cx);
    cx.write_to_clipboard(item);
    cx.simulate_keystrokes(paste_key(false));
    draw(cx);
    chat.read_with(cx, |chat, cx| {
        assert_eq!(chat.input.read(cx).value().as_ref(), text);
        assert!(
            chat.draft.files.is_empty(),
            "mixed clipboard text must not become a second file"
        );
        assert_eq!(chat.draft.images.len(), 1);
    });
}
