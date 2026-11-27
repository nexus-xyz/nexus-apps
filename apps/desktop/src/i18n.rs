//! The interface's language (Settings › Appearance). Text in the code stays English and
//! is its own key: `tr("Theme")` looks it up in `locales/app.yml` under the current locale, and an entry that is not
//! there shows the English. rust-i18n keeps one locale for the whole process, the one gpui-kit's components read too, so
//! [`set`] switches their labels (where they have the language) along with ours.

use gpui_kit::{App, SharedString};

rust_i18n::i18n!("locales", fallback = "en");

/// The languages the interface ships in: its locale code, and its name in itself.
pub const LANGUAGES: [(&str, &str); 3] = [("en", "English"), ("es", "Español"), ("pt-BR", "Português (Brasil)")];

/// `english` in the current language, else `english`.
pub fn tr(english: &'static str) -> SharedString {
    // The macro's own lookup, by exact text: `t!` would show `es.<key>` for a missing entry, and needs `i18n!` at the
    // crate root.
    match _rust_i18n_try_translate(&rust_i18n::locale(), english) {
        Some(text) => text.into_owned().into(),
        None => english.into(),
    }
}

/// The locale the interface uses: the person's choice in `settings.json` (`language`), else the computer's language
/// (`system`, as the OS gives it: `es-MX`, `pt_BR.UTF-8`, `en-US`, `C`…), else English. Always one of [`LANGUAGES`].
/// A saved code this build does not ship (a later build's) follows the computer. Every Spanish locale gets `es`, and
/// every Portuguese one gets `pt-BR`: closer than English for a reader in Portugal.
fn resolve(chosen: Option<&str>, system: Option<&str>) -> &'static str {
    let shipped = |code: &str| {
        LANGUAGES
            .iter()
            .map(|&(shipped, _)| shipped)
            .find(|shipped| shipped.eq_ignore_ascii_case(code))
    };
    if let Some(code) = chosen.and_then(shipped) {
        return code;
    }
    // The language and region, without the encoding or modifier (`pt_BR.UTF-8`, `ca_ES@valencia`).
    let system = system
        .unwrap_or_default()
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    let language = system.split('-').next().unwrap_or_default();
    shipped(&system)
        .or_else(|| match language.to_ascii_lowercase().as_str() {
            "es" => Some("es"),
            "pt" => Some("pt-BR"),
            _ => None,
        })
        .unwrap_or("en")
}

/// Use the saved language, before the first window opens.
pub fn start() {
    let chosen = crate::desktop_settings::load().language;
    rust_i18n::set_locale(resolve(chosen.as_deref(), sys_locale::get_locale().as_deref()));
}

/// The person's choice; None follows the computer.
pub fn chosen() -> Option<String> {
    crate::desktop_settings::load().language
}

/// Save `code` (None: follow the computer) and redraw every window in it. Text a view built once (an input's
/// placeholder, a menu) changes when it is built again.
pub fn set(code: Option<&'static str>, cx: &mut App) -> Result<(), String> {
    crate::desktop_settings::update(|settings| {
        settings.language = code.map(str::to_string);
        Ok(())
    })?;
    rust_i18n::set_locale(resolve(code, sys_locale::get_locale().as_deref()));
    cx.refresh_windows();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every literal passed to `tr` in `src/` has both translations: a string added in English only fails here, not on screen.
    #[test]
    fn every_text_in_tr_is_translated() {
        let mut missing = Vec::new();
        let mut seen = 0;
        for (path, text) in tr_texts(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))) {
            seen += 1;
            for (code, _) in &LANGUAGES[1..] {
                if _rust_i18n_try_translate(code, &text).is_none() {
                    missing.push(format!("{code}: {text:?} ({path})"));
                }
            }
        }
        assert!(seen > 0, "found no literal passed to tr: the scan reads nothing");
        assert!(missing.is_empty(), "not in locales/app.yml:\n{}", missing.join("\n"));
    }

    #[test]
    fn the_locale_is_the_choice_else_the_computers_else_english() {
        let cases = [
            ((Some("es"), Some("en-US")), "es"),
            ((Some("pt-br"), None), "pt-BR"),
            ((Some("fr"), Some("es-419")), "es"),
            ((None, Some("es-MX")), "es"),
            ((None, Some("pt_BR.UTF-8")), "pt-BR"),
            ((None, Some("pt-PT")), "pt-BR"),
            ((None, Some("en_GB@euro")), "en"),
            ((None, Some("fr-FR")), "en"),
            ((None, Some("C")), "en"),
            ((None, Some("")), "en"),
            ((None, None), "en"),
        ];
        for ((chosen, system), want) in cases {
            assert_eq!(resolve(chosen, system), want, "{chosen:?} {system:?}");
        }
    }

    #[test]
    fn a_text_without_an_entry_shows_the_english() {
        assert_eq!(_rust_i18n_try_translate("es", "No entry has this text"), None);
        assert_eq!(_rust_i18n_try_translate("es", "Language").as_deref(), Some("Idioma"));
        assert_eq!(_rust_i18n_try_translate("pt-BR", "Language").as_deref(), Some("Idioma"));
    }

    #[test]
    fn the_scan_reads_rust_string_escapes() {
        let source = "x.child(tr(\"a \\\"b\\\" \\\n             c\")); tr(other); str(\"x\"); tr(\n    \"d\")";
        assert_eq!(literals(source), ["a \"b\" c", "d"]);
    }

    /// The literals passed to `tr` in every `.rs` file under `dir`, with the file each is in. This file is not read: it
    /// defines `tr` and its scan, and shows no text.
    fn tr_texts(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).expect("src/ reads") {
            let path = entry.expect("an entry reads").path();
            if path.is_dir() {
                found.extend(tr_texts(&path));
            } else if path.extension().is_some_and(|ext| ext == "rs") && !path.ends_with("src/i18n.rs") {
                let source = std::fs::read_to_string(&path).expect("a source file reads");
                let name = path.display().to_string();
                found.extend(literals(&source).into_iter().map(|text| (name.clone(), text)));
            }
        }
        found
    }

    /// The string literals passed straight to `tr(`, unescaped as Rust reads them.
    fn literals(source: &str) -> Vec<String> {
        let mut texts = Vec::new();
        for (start, _) in source.match_indices("tr(") {
            // `str(`, `attr(` and the like are other calls.
            if source[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            // rustfmt moves a long literal to the next line.
            let Some(literal) = source[start + 3..].trim_start().strip_prefix('"') else {
                continue;
            };
            let mut text = String::new();
            let mut chars = literal.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some('\n') => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                        Some(other) => text.push(other),
                        None => {}
                    },
                    c => text.push(c),
                }
            }
            texts.push(text);
        }
        texts
    }
}
