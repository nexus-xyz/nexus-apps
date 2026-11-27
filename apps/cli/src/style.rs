//! The `nexus` CLI's design system (`format.ts` is its twin: the same tokens, glyphs and layouts, so both print the
//! same text). Widths count UTF-16 code units, as JavaScript's `length` and `padEnd` do, so columns line up exactly as
//! the TypeScript CLI's.
//!
//! - **Tokens**, not colours, from brand v5.0's Nexus AI product layer (`brand/v5.0/nexus-ai/SPEC.md`): `dim`
//!   (`text-2`, secondary text), `faint` (`text-3`: rules, hints, empty cells), `accent` (the strong ink: the
//!   selection, commands, the brand; every grey is neutral and no hue decorates), `ok`, `warn`, `err` (the app's status
//!   colours), and each model provider's mark (Nexus UI's `model-logo.tsx`). Each tone has a dark value and a light
//!   one; dark is the default, and `COLORFGBG` or the session's own question (`with_light`) picks light. They print in
//!   truecolor, 256 or 16 colours (`COLORTERM`, `TERM`, `FORCE_COLOR=1|2|3`), and not at all when stdout is not a
//!   terminal, with `NO_COLOR`, or with `--no-color`. `--json` never goes through here.
//! - **Glyphs** (✓ ✕ ! › → · ❯ ● ✱ ■ □ ━ ─ …) fall back to ASCII when the locale is not UTF-8.
//! - **Layouts**: `list` (aligned columns with `·`/`❯` row marks, sentence-case dim headers, groups with spacing, a
//!   total, width-aware truncation), `pairs` (label, value), `stack` (dim labels over bright values), `meter` (■■■□),
//!   `budget_bar` (a slim bar, green → amber → red), `meta` (a dim `a · b · c` line), `legend` and `key_hints`.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// How many colours the terminal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    C16,
    C256,
    True,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub ok: &'static str,
    pub fail: &'static str,
    pub warn: &'static str,
    pub info: &'static str,
    pub arrow: &'static str,
    pub bullet: &'static str,
    pub select: &'static str,
    pub dot: &'static str,
    pub star: &'static str,
    pub meter_on: &'static str,
    pub meter_off: &'static str,
    pub bar_on: &'static str,
    pub bar_off: &'static str,
    pub rule: &'static str,
    pub ellipsis: &'static str,
    pub updown: &'static str,
    pub enter: &'static str,
    pub cursor: &'static str,
}

pub const UNICODE_GLYPHS: Glyphs = Glyphs {
    ok: "✓",
    fail: "✕",
    warn: "!",
    info: "›",
    arrow: "→",
    bullet: "·",
    select: "❯",
    dot: "●",
    star: "✱",
    meter_on: "■",
    meter_off: "□",
    bar_on: "━",
    bar_off: "─",
    rule: "─",
    ellipsis: "…",
    updown: "↑↓",
    enter: "↵",
    cursor: "▏",
};

pub const ASCII_GLYPHS: Glyphs = Glyphs {
    ok: "+",
    fail: "x",
    warn: "!",
    info: ">",
    arrow: "->",
    bullet: "-",
    select: ">",
    dot: "*",
    star: "*",
    meter_on: "#",
    meter_off: ".",
    bar_on: "=",
    bar_off: "-",
    rule: "-",
    ellipsis: "...",
    updown: "up/down",
    enter: "enter",
    cursor: "|",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Dim,
    Faint,
    Accent,
    Ok,
    Warn,
    Err,
}

/// Each token: its truecolor value on a dark terminal and on a light one, and its 16-colour SGR codes (open, close).
/// 16 colours cannot name the strong ink, so `accent` is bold there.
fn token(tone: Tone) -> ((&'static str, &'static str), (u8, u8)) {
    match tone {
        Tone::Dim => (("a1a1a6", "5c5c60"), (2, 22)),
        Tone::Faint => (("8a8a8e", "6e6e73"), (90, 39)),
        Tone::Accent => (("f5f5f7", "000000"), (1, 22)),
        Tone::Ok => (("30d158", "1a7f37"), (32, 39)),
        Tone::Warn => (("ffd60a", "8a5300"), (33, 39)),
        Tone::Err => (("ff453a", "b42318"), (31, 39)),
    }
}

/// A tone's truecolor value on this ground.
fn hex_of(tone: Tone, light: bool) -> &'static str {
    let (dark, on_light) = token(tone).0;
    if light { on_light } else { dark }
}

/// Model providers as Nexus UI marks them (`model-logo.tsx`: simple-icons' brand colours): name, colour, 16-colour
/// code. OpenAI and xAI have no licensed mark there, so they stay neutral (`dim`) here too; so does any provider we
/// do not know. Nexus is one ink, never a hue: it prints in the accent.
fn provider_info(provider: &str) -> Option<(&'static str, Option<(&'static str, u8)>)> {
    Some(match provider {
        "anthropic" => ("Anthropic", Some(("d97757", 33))),
        "openai" => ("OpenAI", None),
        "google" => ("Google", Some(("8e75b2", 35))),
        "xai" => ("xAI", None),
        "deepseek" => ("DeepSeek", Some(("5786fe", 34))),
        "meta" => ("Meta", Some(("0467df", 34))),
        "mistral" => ("Mistral", Some(("fa520f", 33))),
        "alibaba" => ("Alibaba", Some(("6950ef", 35))),
        "qwen" => ("Qwen", Some(("6950ef", 35))),
        "nexus" => ("Nexus", None),
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct Style {
    pub color: bool,
    pub depth: Depth,
    pub unicode: bool,
    /// The terminal's background is light: each tone takes its light value.
    pub light: bool,
    /// The terminal's width in columns; `None` when stdout is not a terminal (nothing is truncated then).
    pub width: Option<usize>,
    pub g: Glyphs,
    /// Symbols, styled: success, failure, warning, info, arrow, bullet, the selected row's mark.
    pub ok: String,
    pub fail: String,
    pub warn: String,
    pub info: String,
    pub arrow: String,
    pub bullet: String,
    pub select: String,
}

fn esc(code: &str) -> String {
    format!("\x1b[{code}m")
}

/// `open text close`, reopening `open` after any `close` inside (a nested style), as chalk does.
fn wrap_with(open: &str, close: &str, text: &str) -> String {
    format!(
        "{open}{}{close}",
        text.split(close).collect::<Vec<_>>().join(&format!("{close}{open}"))
    )
}

fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let at = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    (at(0), at(2), at(4))
}

/// The nearest of xterm's 256 colours: the 6×6×6 cube or the grey ramp, whichever is closer.
pub fn ansi256(hex: &str) -> u8 {
    let (r, g, b) = hex_rgb(hex);
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    const LEVELS: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    let step = |v: f64| -> usize {
        if v < 48.0 {
            0
        } else if v < 115.0 {
            1
        } else {
            ((v - 35.0) / 40.0).floor() as usize
        }
    };
    let (ir, ig, ib) = (step(r), step(g), step(b));
    let cube = [LEVELS[ir], LEVELS[ig], LEVELS[ib]];
    let average = (r + g + b) / 3.0;
    let grey = if average > 238.0 {
        23.0
    } else {
        js_round((average - 8.0) / 10.0).max(0.0)
    };
    let grey_value = 8.0 + 10.0 * grey;
    let distance = |c: [f64; 3]| (r - c[0]).powi(2) + (g - c[1]).powi(2) + (b - c[2]).powi(2);
    if distance([grey_value; 3]) < distance(cube) {
        232 + grey as u8
    } else {
        (16 + 36 * ir + 6 * ig + ib) as u8
    }
}

fn foreground(hex: &str, depth: Depth, fallback: (u8, u8)) -> (String, String) {
    match depth {
        Depth::True => {
            let (r, g, b) = hex_rgb(hex);
            (esc(&format!("38;2;{r};{g};{b}")), esc("39"))
        }
        Depth::C256 => (esc(&format!("38;5;{}", ansi256(hex))), esc("39")),
        Depth::C16 => (esc(&fallback.0.to_string()), esc(&fallback.1.to_string())),
    }
}

impl Style {
    /// 16 colours (when `color`), UTF-8 glyphs, no width: tests, and what a run starts from.
    pub fn new(color: bool) -> Style {
        Style::with(color, Depth::C16, true, None)
    }

    pub fn with(color: bool, depth: Depth, unicode: bool, width: Option<usize>) -> Style {
        let g = if unicode { UNICODE_GLYPHS } else { ASCII_GLYPHS };
        let mut style = Style {
            color,
            depth,
            unicode,
            light: false,
            width: width.filter(|width| *width > 0),
            g,
            ok: String::new(),
            fail: String::new(),
            warn: String::new(),
            info: String::new(),
            arrow: String::new(),
            bullet: String::new(),
            select: String::new(),
        };
        style.symbols();
        style
    }

    /// The same style on a light (`true`) or dark terminal.
    pub fn with_light(mut self, light: bool) -> Style {
        self.light = light;
        self.symbols();
        self
    }

    fn symbols(&mut self) {
        let g = self.g;
        self.ok = self.green(g.ok);
        self.fail = self.red(g.fail);
        self.warn = self.yellow(g.warn);
        self.info = self.dim(g.info);
        self.arrow = self.dim(g.arrow);
        self.bullet = self.dim(g.bullet);
        self.select = self.accent(g.select);
    }

    fn sgr(&self, open: u8, close: u8, text: &str) -> String {
        if self.color {
            wrap_with(&esc(&open.to_string()), &esc(&close.to_string()), text)
        } else {
            text.to_string()
        }
    }

    pub fn tone(&self, tone: Tone, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        let (open, close) = foreground(hex_of(tone, self.light), self.depth, token(tone).1);
        wrap_with(&open, &close, text)
    }

    /// An exact colour (`rrggbb`), with a 16-colour fallback: the budget bar's gradient.
    pub fn rgb(&self, hex: &str, fallback: Tone, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        let (open, close) = foreground(hex, self.depth, token(fallback).1);
        wrap_with(&open, &close, text)
    }

    /// Text in a model provider's colour (neutral for providers without a mark; Nexus in the accent).
    pub fn provider(&self, provider: &str, text: &str) -> String {
        if provider == "nexus" {
            return self.accent(text);
        }
        let Some((hex, sgr)) = provider_info(provider).and_then(|(_, mark)| mark) else {
            return self.dim(text);
        };
        if !self.color {
            return text.to_string();
        }
        let (open, close) = foreground(hex, self.depth, (sgr, 39));
        wrap_with(&open, &close, text)
    }

    pub fn bold(&self, text: &str) -> String {
        self.sgr(1, 22, text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.tone(Tone::Dim, text)
    }
    pub fn faint(&self, text: &str) -> String {
        self.tone(Tone::Faint, text)
    }
    pub fn underline(&self, text: &str) -> String {
        self.sgr(4, 24, text)
    }
    pub fn accent(&self, text: &str) -> String {
        self.tone(Tone::Accent, text)
    }
    /// Legacy names: `red` is `err`, `green` is `ok`, `yellow` is `warn`, `cyan` is `accent`.
    pub fn red(&self, text: &str) -> String {
        self.tone(Tone::Err, text)
    }
    pub fn green(&self, text: &str) -> String {
        self.tone(Tone::Ok, text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.tone(Tone::Warn, text)
    }
    pub fn cyan(&self, text: &str) -> String {
        self.tone(Tone::Accent, text)
    }
}

/// The style for a run: colour as decided, its depth and the glyphs from the environment, the terminal's width.
pub fn style_for(color: bool, env: &dyn Fn(&str) -> Option<String>, width: Option<usize>) -> Style {
    Style::with(color, color_depth(env), unicode_locale(env), width).with_light(light_background(env))
}

/// Whether the terminal says its background is light: `COLORFGBG` (`15;0` is dark, `0;15` light), which rxvt, Konsole
/// and iTerm set. Without it, dark: the brand is dark first.
pub fn light_background(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let value = env("COLORFGBG").unwrap_or_default();
    match value.rsplit(';').next().and_then(|bg| bg.trim().parse::<u8>().ok()) {
        Some(bg) => bg == 7 || bg >= 9,
        None => false,
    }
}

/// Whether to color: a terminal only, never with `NO_COLOR` or `--no-color`, always with `FORCE_COLOR` (unless "0").
pub fn should_color(is_tty: bool, env: &dyn Fn(&str) -> Option<String>, argv: &[String]) -> bool {
    if argv.iter().any(|arg| arg == "--no-color") {
        return false;
    }
    if env("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if let Some(force) = env("FORCE_COLOR") {
        return force != "0";
    }
    if env("TERM").as_deref() == Some("dumb") {
        return false;
    }
    is_tty
}

/// How many colours the terminal shows: `FORCE_COLOR=3|2`, `COLORTERM=truecolor`, a `-direct` or `256color` TERM.
pub fn color_depth(env: &dyn Fn(&str) -> Option<String>) -> Depth {
    match env("FORCE_COLOR").as_deref() {
        Some("3") => return Depth::True,
        Some("2") => return Depth::C256,
        _ => {}
    }
    let colorterm = env("COLORTERM").unwrap_or_default().to_lowercase();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return Depth::True;
    }
    let term = env("TERM").unwrap_or_default().to_lowercase();
    if term.ends_with("-direct") || term.contains("truecolor") {
        return Depth::True;
    }
    let program = env("TERM_PROGRAM").unwrap_or_default();
    if ["iTerm.app", "WezTerm", "ghostty", "vscode"].contains(&program.as_str())
        || env("WT_SESSION").is_some_and(|value| !value.is_empty())
    {
        return Depth::True;
    }
    if term.contains("256") {
        return Depth::C256;
    }
    Depth::C16
}

/// Whether the locale is UTF-8 (`LC_ALL`, else `LC_CTYPE`, else `LANG`); with none set, assume it is.
pub fn unicode_locale(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| env(name).filter(|value| !value.is_empty()));
    match locale {
        None => true,
        Some(locale) => {
            let lower = locale.to_lowercase();
            lower.contains("utf8") || lower.contains("utf-8")
        }
    }
}

/// JavaScript's `length` of the text without ANSI escapes (UTF-16 code units).
pub fn visible_length(text: &str) -> usize {
    js_len(&strip_ansi(text))
}

pub fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The text split into escape codes and runs of text, in order (`true`: an escape code).
fn ansi_parts(text: &str) -> Vec<(bool, String)> {
    let mut parts: Vec<(bool, String)> = Vec::new();
    let mut plain = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            let mut probe = chars.clone();
            probe.next();
            let mut code = String::from("\x1b[");
            let mut ok = false;
            for d in probe {
                code.push(d);
                if d == 'm' {
                    ok = true;
                    break;
                }
                if !(d.is_ascii_digit() || d == ';') {
                    break;
                }
            }
            if ok {
                for _ in 0..code.chars().count() - 1 {
                    chars.next();
                }
                if !plain.is_empty() {
                    parts.push((false, std::mem::take(&mut plain)));
                }
                parts.push((true, code));
                continue;
            }
        }
        plain.push(c);
    }
    if !plain.is_empty() {
        parts.push((false, plain));
    }
    parts
}

/// The text without its ANSI escape codes.
pub fn strip_ansi(text: &str) -> String {
    ansi_parts(text)
        .into_iter()
        .filter(|(code, _)| !code)
        .map(|(_, part)| part)
        .collect()
}

/// The first `units` UTF-16 code units of `text` (`text.slice(0, units)`, never splitting a character).
fn js_slice(text: &str, units: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in text.chars() {
        count += c.len_utf16();
        if count > units {
            break;
        }
        out.push(c);
    }
    out
}

/// At most `width` visible characters, ending in `…` when cut. Escape codes are kept (all of them, so every style
/// still closes); only text is dropped.
pub fn truncate(text: &str, width: usize, ellipsis: &str) -> String {
    if visible_length(text) <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(js_len(ellipsis));
    let mut out = String::new();
    let mut shown = 0;
    let mut cut = false;
    for (code, part) in ansi_parts(text) {
        if code {
            out.push_str(&part);
            continue;
        }
        if cut {
            continue;
        }
        let len = js_len(&part);
        if shown + len <= keep {
            out.push_str(&part);
            shown += len;
        } else {
            out.push_str(&js_slice(&part, keep - shown));
            out.push_str(ellipsis);
            shown = keep;
            cut = true;
        }
    }
    out
}

/// `text.padEnd(width)` in UTF-16 code units.
pub fn pad_end(text: &str, width: usize) -> String {
    let len = js_len(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(len)))
}

/// `text.padStart(width)` in UTF-16 code units.
pub fn pad_start(text: &str, width: usize) -> String {
    let len = js_len(text);
    format!("{}{text}", " ".repeat(width.saturating_sub(len)))
}

fn pad(text: &str, width: usize, right: bool) -> String {
    let gap = " ".repeat(width.saturating_sub(visible_length(text)));
    if right {
        format!("{gap}{text}")
    } else {
        format!("{text}{gap}")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Column<'a> {
    pub header: &'a str,
    pub right: bool,
    /// This column gives way (its cells end in `…`) when a row is wider than the terminal.
    pub flex: bool,
}

pub const fn col(header: &str) -> Column<'_> {
    Column {
        header,
        right: false,
        flex: false,
    }
}

pub const fn right(header: &str) -> Column<'_> {
    Column {
        header,
        right: true,
        flex: false,
    }
}

pub const fn flex(header: &str) -> Column<'_> {
    Column {
        header,
        right: false,
        flex: true,
    }
}

/// A row of `list`: its cells, and how it is marked (`·` unless `selected` (`❯`) or another glyph), or muted.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cells: Vec<String>,
    pub selected: bool,
    /// A glyph of one character, styled, in place of the bullet.
    pub mark: Option<String>,
    /// Shown dim: coming soon, archived, not usable.
    pub muted: bool,
}

impl From<Vec<String>> for Row {
    fn from(cells: Vec<String>) -> Row {
        Row {
            cells,
            ..Row::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Group {
    /// A line above the group's rows, with a blank line before it.
    pub title: Option<String>,
    pub rows: Vec<Row>,
}

const GAP: &str = "   ";

fn trim_end_js(text: &str) -> String {
    text.trim_end().to_string()
}

/// Rows in aligned columns: a dim, sentence-case header; a `·` before each row (`❯` and the accent for the selected
/// one); groups under their own title with a blank line between; an optional total under a faint rule. On a terminal
/// narrower than the rows, the `flex` column's cells end in `…`.
pub fn grouped_list(style: &Style, columns: &[Column], groups: &[Group], footer: Option<Vec<String>>) -> Vec<String> {
    let headers: Vec<String> = columns.iter().map(|column| column.header.to_string()).collect();
    let mut all: Vec<&Vec<String>> = vec![&headers];
    all.extend(groups.iter().flat_map(|group| group.rows.iter().map(|row| &row.cells)));
    if let Some(footer) = &footer {
        all.push(footer);
    }
    let mut widths: Vec<usize> = (0..columns.len())
        .map(|i| {
            all.iter()
                .map(|cells| cells.get(i).map(|cell| visible_length(cell)).unwrap_or(0))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let total = |widths: &[usize]| 2 + widths.iter().sum::<usize>() + GAP.len() * widths.len().saturating_sub(1);
    let flex_at = columns.iter().position(|column| column.flex);
    if let (Some(width), Some(at)) = (style.width, flex_at) {
        let wide = total(&widths);
        if wide > width {
            widths[at] = widths[at].saturating_sub(wide - width).max(6);
        }
    }
    let line = |cells: &[String]| -> String {
        let joined = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = widths.get(i).copied().unwrap_or(0);
                let fitted = if Some(i) == flex_at {
                    truncate(cell, width, style.g.ellipsis)
                } else {
                    cell.clone()
                };
                pad(&fitted, width, columns.get(i).is_some_and(|c| c.right))
            })
            .collect::<Vec<_>>()
            .join(GAP);
        trim_end_js(&joined)
    };
    let mut out: Vec<String> = Vec::new();
    if headers.iter().any(|header| !header.is_empty()) {
        out.push(format!("  {}", style.dim(&line(&headers))));
    }
    for group in groups {
        if let Some(title) = &group.title {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push(format!("  {title}"));
        }
        for row in &group.rows {
            let paint = |cell: &String| -> String {
                if cell.is_empty() {
                    cell.clone()
                } else if row.selected {
                    style.accent(cell)
                } else if row.muted {
                    style.dim(cell)
                } else {
                    cell.clone()
                }
            };
            let mark = if row.selected {
                style.select.clone()
            } else {
                row.mark.clone().unwrap_or_else(|| style.bullet.clone())
            };
            let cells: Vec<String> = row.cells.iter().map(paint).collect();
            out.push(format!("{mark} {}", line(&cells)));
        }
    }
    if let Some(footer) = footer {
        out.push(format!(
            "  {}",
            style.faint(&style.g.rule.repeat(total(&widths).saturating_sub(2)))
        ));
        out.push(format!("  {}", style.bold(&line(&footer))));
    }
    out
}

/// `grouped_list` with one group and no title.
pub fn list(style: &Style, columns: &[Column], rows: Vec<Row>, footer: Option<Vec<String>>) -> Vec<String> {
    grouped_list(style, columns, &[Group { title: None, rows }], footer)
}

/// Plain rows (no marks of their own) for `list`.
pub fn rows_of(cells: Vec<Vec<String>>) -> Vec<Row> {
    cells.into_iter().map(Row::from).collect()
}

/// Dim labels over bright values, in columns: `Input   Output` over `$5.00   $25.00`.
pub fn stack(style: &Style, entries: &[(String, String)]) -> Vec<String> {
    let widths: Vec<usize> = entries
        .iter()
        .map(|(label, value)| visible_length(label).max(visible_length(value)))
        .collect();
    let line = |cells: Vec<String>| -> String {
        trim_end_js(
            &cells
                .iter()
                .enumerate()
                .map(|(i, cell)| pad(cell, widths.get(i).copied().unwrap_or(0), false))
                .collect::<Vec<_>>()
                .join(GAP),
        )
    };
    vec![
        format!(
            "  {}",
            style.dim(&line(entries.iter().map(|(label, _)| label.clone()).collect()))
        ),
        format!(
            "  {}",
            line(entries.iter().map(|(_, value)| style.bold(value)).collect())
        ),
    ]
}

/// `a · b · c`, dim: a header line's metadata. Empty parts are left out.
pub fn meta(style: &Style, parts: &[Option<String>]) -> String {
    let parts: Vec<&str> = parts
        .iter()
        .filter_map(|part| part.as_deref())
        .filter(|part| !part.is_empty())
        .collect();
    style.dim(&parts.join(&format!(" {} ", style.g.bullet)))
}

/// The CLI's name as a header shows it.
pub fn brand(style: &Style) -> String {
    style.accent(&style.bold("nexus"))
}

/// ok below 75 %, warn below 90 %, err from there.
pub fn level_tone(fraction: f64) -> Tone {
    if fraction < 0.75 {
        Tone::Ok
    } else if fraction < 0.9 {
        Tone::Warn
    } else {
        Tone::Err
    }
}

fn filled_cells(fraction: f64, width: usize) -> usize {
    let fraction = if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    };
    (js_round(fraction * width as f64).max(0.0) as usize).min(width)
}

/// A block meter, `■■■□□□□□□□`: the filled part bright (in `tone` when given), the rest faint.
pub fn meter(style: &Style, fraction: f64, width: usize, tone: Option<Tone>) -> String {
    let filled = filled_cells(fraction, width);
    let on = style.g.meter_on.repeat(filled);
    let on = match tone {
        Some(tone) => style.tone(tone, &on),
        None => on,
    };
    format!("{on}{}", style.faint(&style.g.meter_off.repeat(width - filled)))
}

const GRADIENT: [(f64, &str); 3] = [(0.0, "30d158"), (0.75, "ffd60a"), (1.0, "ff453a")];

/// The colour at `at` (0–1) along green → amber → red.
pub fn gradient_hex(at: f64) -> String {
    let x = at.clamp(0.0, 1.0);
    let mut i = 0;
    while i < GRADIENT.len() - 2 && x > GRADIENT[i + 1].0 {
        i += 1;
    }
    let (from, a) = GRADIENT[i];
    let (to, b) = GRADIENT[i + 1];
    let t = if to == from { 0.0 } else { (x - from) / (to - from) };
    let (ra, ga, ba) = hex_rgb(a);
    let (rb, gb, bb) = hex_rgb(b);
    let mix = |p: u8, q: u8| js_round(f64::from(p) + (f64::from(q) - f64::from(p)) * t) as u8;
    format!("{:02x}{:02x}{:02x}", mix(ra, rb), mix(ga, gb), mix(ba, bb))
}

/// A slim budget bar, `━━━━━━──────────`: the used part runs green → amber → red along the bar (one colour, by how
/// much is used, in 16 colours), the rest faint.
pub fn budget_bar(style: &Style, fraction: f64, width: usize) -> String {
    let filled = filled_cells(fraction, width);
    let on = if !style.color {
        style.g.bar_on.repeat(filled)
    } else if style.depth == Depth::C16 {
        style.tone(level_tone(fraction), &style.g.bar_on.repeat(filled))
    } else {
        (0..filled)
            .map(|i| {
                let at = if width > 1 { i as f64 / (width - 1) as f64 } else { 0.0 };
                style.rgb(&gradient_hex(at), level_tone(fraction), style.g.bar_on)
            })
            .collect()
    };
    format!("{on}{}", style.faint(&style.g.bar_off.repeat(width - filled)))
}

/// `✓ allowed   ✕ not allowed   ✱ coming soon`: each glyph as given, its meaning dim.
pub fn legend(style: &Style, items: &[(String, &str)]) -> String {
    items
        .iter()
        .map(|(glyph, label)| format!("{glyph} {}", style.dim(label)))
        .collect::<Vec<_>>()
        .join("   ")
}

/// `↑↓ select · ↵ confirm · esc cancel`: keys bright, actions and separators dim.
pub fn key_hints(style: &Style, hints: &[(&str, &str)]) -> String {
    hints
        .iter()
        .map(|(key, action)| format!("{key} {}", style.dim(action)))
        .collect::<Vec<_>>()
        .join(&format!(" {} ", style.bullet))
}

/// `` `nexus update` `` spans in the accent, without their backticks, on a terminal; as they are otherwise.
pub fn code(style: &Style, text: &str) -> String {
    if !style.color {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        match after.find(['`', '\n']) {
            Some(end) if end > 0 && after.as_bytes()[end] == b'`' => {
                out.push_str(&rest[..start]);
                out.push_str(&style.accent(&after[..end]));
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The provider of a `provider/model` id, of a bare model name from a known family, or the id itself (as Nexus UI).
pub fn provider_of(model: &str) -> String {
    let id = model.to_lowercase();
    if let Some(slash) = id.find('/')
        && slash > 0
    {
        return id[..slash].to_string();
    }
    let o_digit = id.starts_with('o') && id[1..].starts_with(|c: char| c.is_ascii_digit());
    let family = if id.starts_with("claude") {
        "anthropic"
    } else if id.starts_with("gpt") || o_digit || id.starts_with("codex") {
        "openai"
    } else if id.starts_with("gemini") {
        "google"
    } else if id.starts_with("grok") {
        "xai"
    } else if id.starts_with("deepseek") {
        "deepseek"
    } else if id.starts_with("llama") {
        "meta"
    } else if ["mistral", "codestral", "magistral"].iter().any(|p| id.starts_with(p)) {
        "mistral"
    } else if id.starts_with("qwen") {
        "alibaba"
    } else {
        return id;
    };
    family.to_string()
}

/// A provider's name: `Anthropic`, `OpenAI`; the id itself when we do not know it.
pub fn provider_name(provider: &str) -> String {
    provider_info(provider).map_or_else(|| provider.to_string(), |(name, _)| name.to_string())
}

/// A provider's mark: a dot in its colour.
pub fn provider_mark(style: &Style, provider: &str) -> String {
    style.provider(provider, style.g.dot)
}

/// `label  value` pairs with the labels aligned, indented by two spaces.
pub fn pairs(style: &Style, entries: &[(String, String)]) -> Vec<String> {
    let width = entries.iter().map(|(label, _)| js_len(label)).max().unwrap_or(0);
    entries
        .iter()
        .map(|(label, value)| format!("  {}  {value}", style.dim(&pad_end(label, width))))
        .collect()
}

/// `pairs` from string slices.
pub fn kv(label: &str, value: impl Into<String>) -> (String, String) {
    (label.to_string(), value.into())
}

/// JavaScript's `Number.prototype.toFixed`: the nearest decimal with `digits` fraction digits, ties away from zero.
pub fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return js_number(value);
    }
    if value.abs() >= 1e21 {
        return js_number(value);
    }
    let negative = value < 0.0;
    let exact = format!("{:.*}", digits + 30, value.abs());
    let (int_part, frac_part) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_vec: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes().take(digits))
        .map(|b| b - b'0')
        .collect();
    let rest = &frac_part[digits.min(frac_part.len())..];
    let round_up = rest.as_bytes().first().is_some_and(|&d| d >= b'5');
    if round_up {
        let mut i = digits_vec.len();
        loop {
            if i == 0 {
                digits_vec.insert(0, 1);
                break;
            }
            i -= 1;
            if digits_vec[i] == 9 {
                digits_vec[i] = 0;
            } else {
                digits_vec[i] += 1;
                break;
            }
        }
    }
    let text: String = digits_vec.iter().map(|d| char::from(b'0' + d)).collect();
    let split = text.len() - digits;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&text[..split]);
    if digits > 0 {
        out.push('.');
        out.push_str(&text[split..]);
    }
    out
}

/// JavaScript's `String(number)` for the values the CLI prints: integers without a fraction, otherwise the shortest
/// form that reads back the same.
pub fn js_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if value == 0.0 {
        return "0".into();
    }
    let abs = value.abs();
    if value.fract() == 0.0 && abs < 1e21 {
        return format!("{value:.0}");
    }
    if !(1e-6..1e21).contains(&abs) {
        let text = format!("{value:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
            _ => text,
        };
    }
    format!("{value}")
}

/// JavaScript's `Math.round`: halves go up.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `toLocaleString("en-US")`: thousands separated by commas, at most three fraction digits.
pub fn locale(value: f64) -> String {
    let negative = value < 0.0;
    let fixed = to_fixed(value.abs(), 3);
    let (int_part, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let frac = frac.trim_end_matches('0');
    let mut grouped = String::new();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let sign = if negative && (int_part != "0" || !frac.is_empty()) {
        "-"
    } else {
        ""
    };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// Dollars at the precision the amount needs: budgets in cents, model calls to the micro-dollar.
pub fn usd(value: f64) -> String {
    let abs = value.abs();
    if abs == 0.0 {
        return "$0.00".into();
    }
    if abs >= 100.0 || (value * 100.0 - js_round(value * 100.0)).abs() < 1e-9 {
        return format!("${}", to_fixed(value, 2));
    }
    if abs >= 0.01 {
        return format!("${}", to_fixed(value, 4));
    }
    format!("${}", to_fixed(value, 6))
}

/// Milliseconds since the epoch of an ISO time, as `Date.parse` (None when it is not one).
pub fn parse_ms(iso: &str) -> Option<f64> {
    let parsed = OffsetDateTime::parse(iso, &Rfc3339)
        .ok()
        .or_else(|| OffsetDateTime::parse(&format!("{iso}T00:00:00Z"), &Rfc3339).ok())?;
    Some((parsed.unix_timestamp_nanos() / 1_000_000) as f64)
}

pub fn now_ms() -> f64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as f64
}

/// `new Date(ms).toISOString()`.
pub fn iso(ms: f64) -> String {
    let nanos = (ms as i128) * 1_000_000;
    let at = OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    )
}

pub fn seconds(from: &str, to: Option<&str>) -> String {
    let Some(to) = to.filter(|to| !to.is_empty()) else {
        return "–".into();
    };
    match (parse_ms(from), parse_ms(to)) {
        (Some(a), Some(b)) => {
            let ms = b - a;
            if ms < 1000.0 {
                format!("{} ms", js_number(ms))
            } else {
                format!("{} s", to_fixed(ms / 1000.0, 1))
            }
        }
        _ => "NaN ms".into(),
    }
}

pub fn ago(iso_text: &str) -> String {
    ago_at(iso_text, now_ms())
}

pub fn ago_at(iso_text: &str, now: f64) -> String {
    let fallback = || iso_text.chars().take(10).collect::<String>();
    let Some(at) = parse_ms(iso_text) else {
        return fallback();
    };
    let s = js_round((now - at) / 1000.0);
    if s < 60.0 {
        return format!("{}s ago", js_number(s.max(0.0)));
    }
    if s < 3600.0 {
        return format!("{}m ago", js_number(js_round(s / 60.0)));
    }
    if s < 86400.0 {
        return format!("{}h ago", js_number(js_round(s / 3600.0)));
    }
    fallback()
}

/// `text.charAt(0).toUpperCase() + text.slice(1)`.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `JSON.stringify(text)` for a string.
pub fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_numbers_print_as_javascript_does() {
        assert_eq!(usd(5.0), "$5.00");
        assert_eq!(usd(4.99), "$4.99");
        assert_eq!(usd(4.998107), "$4.9981");
        assert_eq!(usd(0.001438), "$0.001438");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(-3.5), "$-3.50");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(99.995, 2), "100.00");
        assert_eq!(js_number(4.0), "4");
        assert_eq!(js_number(0.1), "0.1");
        assert_eq!(js_number(1e-7), "1e-7");
        assert_eq!(locale(1234567.0), "1,234,567");
        assert_eq!(locale(12.0), "12");
        assert_eq!(locale(1234.5678), "1,234.568");
    }

    #[test]
    fn lists_mark_rows_group_and_total() {
        let style = Style::new(false);
        let lines = grouped_list(
            &style,
            &[col("Model"), right("Input / 1M"), col("Allowed")],
            &[
                Group {
                    title: Some("Anthropic".into()),
                    rows: vec![
                        Row::from(vec!["claude-haiku-5-5".into(), "$1.00".into(), style.ok.clone()]),
                        Row {
                            cells: vec!["claude-opus-5-5".into(), "$5.00".into(), style.ok.clone()],
                            selected: true,
                            ..Row::default()
                        },
                    ],
                },
                Group {
                    title: Some("OpenAI".into()),
                    rows: vec![Row {
                        cells: vec!["gpt-6".into(), "$2.50".into(), style.fail.clone()],
                        mark: Some(style.warn.clone()),
                        ..Row::default()
                    }],
                },
            ],
            Some(vec!["Total".into(), "$8.50".into(), String::new()]),
        );
        assert_eq!(
            lines,
            vec![
                "  Model              Input / 1M   Allowed",
                "",
                "  Anthropic",
                "· claude-haiku-5-5        $1.00   ✓",
                "❯ claude-opus-5-5         $5.00   ✓",
                "",
                "  OpenAI",
                "! gpt-6                   $2.50   ✕",
                "  ───────────────────────────────────────",
                "  Total                   $8.50",
            ]
        );
    }

    #[test]
    fn narrow_terminals_cut_the_flex_column() {
        let style = Style::with(false, Depth::C16, true, Some(30));
        let lines = list(
            &style,
            &[col("Id"), flex("Title")],
            rows_of(vec![vec![
                "chs_1".into(),
                "A rather long title that does not fit".into(),
            ]]),
            None,
        );
        assert_eq!(lines[1], "· chs_1   A rather long title…");
        assert!(lines.iter().all(|line| visible_length(line) <= 30));
        assert_eq!(
            truncate(&Style::new(true).bold("abcdef"), 4, "…"),
            "\x1b[1mabc…\x1b[22m"
        );
    }

    #[test]
    fn meters_bars_and_glyphs() {
        let plain = Style::new(false);
        assert_eq!(meter(&plain, 0.3, 10, None), "■■■□□□□□□□");
        assert_eq!(budget_bar(&plain, 0.5, 8), "━━━━────");
        assert_eq!(budget_bar(&plain, 2.0, 4), "━━━━");
        let ascii = Style::with(false, Depth::C16, false, None);
        assert_eq!(meter(&ascii, 0.5, 4, None), "##..");
        assert_eq!(ascii.bullet, "-");
        assert_eq!(level_tone(0.5), Tone::Ok);
        assert_eq!(level_tone(0.8), Tone::Warn);
        assert_eq!(level_tone(0.95), Tone::Err);
        assert_eq!(gradient_hex(0.0), "30d158");
        assert_eq!(gradient_hex(1.0), "ff453a");
        assert_eq!(ansi256("5ac8f0"), 81);
        assert_eq!(ansi256("8a8f98"), 246);
        let truecolor = Style::with(true, Depth::True, true, None);
        assert_eq!(truecolor.accent("x"), "\x1b[38;2;245;245;247mx\x1b[39m");
        assert_eq!(
            truecolor.clone().with_light(true).accent("x"),
            "\x1b[38;2;0;0;0mx\x1b[39m"
        );
        assert_eq!(
            Style::with(true, Depth::C256, true, None).accent("x"),
            "\x1b[38;5;255mx\x1b[39m"
        );
        assert_eq!(Style::new(true).accent("x"), "\x1b[1mx\x1b[22m");
        let env = |value: &'static str| move |name: &str| (name == "COLORFGBG").then(|| value.to_string());
        assert!(light_background(&env("0;15")) && !light_background(&env("15;0")) && !light_background(&|_| None));
        assert_eq!(
            stack(&plain, &[kv("Input", "$5.00"), kv("Cached input", "$0.50")]),
            vec!["  Input   Cached input", "  $5.00   $0.50"]
        );
        assert_eq!(
            meta(&plain, &[Some("0.4.1".into()), None, Some("acme".into())]),
            "0.4.1 · acme"
        );
        assert_eq!(
            code(&Style::new(true), "Run `nexus update`."),
            "Run \x1b[1mnexus update\x1b[22m."
        );
        assert_eq!(code(&plain, "Run `nexus update`."), "Run `nexus update`.");
    }

    #[test]
    fn providers_and_terminals() {
        assert_eq!(provider_of("anthropic/claude-opus-5-5"), "anthropic");
        assert_eq!(provider_of("gpt-6-mini"), "openai");
        assert_eq!(provider_of("o4-mini"), "openai");
        assert_eq!(provider_of("grok-5"), "xai");
        assert_eq!(provider_name("openai"), "OpenAI");
        assert_eq!(provider_name("acme"), "acme");
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(color_depth(&env(&[("COLORTERM", "truecolor")])), Depth::True);
        assert_eq!(color_depth(&env(&[("TERM", "xterm-256color")])), Depth::C256);
        assert_eq!(color_depth(&env(&[("TERM", "xterm")])), Depth::C16);
        assert_eq!(color_depth(&env(&[("FORCE_COLOR", "3")])), Depth::True);
        assert!(unicode_locale(&env(&[])));
        assert!(unicode_locale(&env(&[("LANG", "en_US.UTF-8")])));
        assert!(!unicode_locale(&env(&[("LC_ALL", "C")])));
    }

    #[test]
    fn colors_only_a_terminal() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert!(should_color(true, &env(&[]), &[]));
        assert!(!should_color(false, &env(&[]), &[]));
        assert!(!should_color(true, &env(&[("NO_COLOR", "1")]), &[]));
        assert!(!should_color(true, &env(&[]), &["--no-color".into()]));
        assert!(!should_color(true, &env(&[("TERM", "dumb")]), &[]));
        assert!(should_color(false, &env(&[("FORCE_COLOR", "1")]), &[]));
        assert!(!should_color(true, &env(&[("FORCE_COLOR", "0")]), &[]));
        let styled = Style::new(true);
        assert_eq!(visible_length(&styled.bold("abc")), 3);
    }

    #[test]
    fn times_read_like_the_typescript_cli() {
        let now = parse_ms("2026-10-18T12:00:00Z").expect("time");
        assert_eq!(ago_at("2026-10-18T11:59:30Z", now), "30s ago");
        assert_eq!(ago_at("2026-10-18T11:00:00.000Z", now), "1h ago");
        assert_eq!(ago_at("2026-10-10T11:00:00Z", now), "2026-10-10");
        assert_eq!(
            seconds("2026-10-18T12:00:00Z", Some("2026-10-18T12:00:02.500Z")),
            "2.5 s"
        );
        assert_eq!(seconds("2026-10-18T12:00:00Z", None), "–");
        assert_eq!(iso(now), "2026-10-18T12:00:00.000Z");
    }
}
